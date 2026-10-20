//! Built-in Speeduino simulator for demos, development and tests without an ECU.
//!
//! A small engine model produces a lap-like cycle (idle, acceleration through
//! the gears, braking with fuel cut). Each state is encoded into a real 130-byte
//! `A` packet and decoded by the production parser, so the full pipeline runs.

use super::{Tx, poller, send_link};
use crate::config::AppConfig;
use crate::hub::SourceEvent;
use crate::protocol::EcuProtocol;
use crate::protocol::speeduino::{SimValues, encode};
use crate::sample::now_ms;
use crate::shared::LinkStatus;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

pub const SIGNATURE: &str = "speeduino 202501 (simulated)";
const ENGINE_RUN: u8 = 1 << 0;
const ENGINE_WARMUP: u8 = 1 << 3;
const ENGINE_TPS_ACCEL: u8 = 1 << 4;
const STATUS2_SYNC: u8 = 1 << 7;
const STATUS2_SOFT_LIMIT: u8 = 1 << 3;

/// Engine state at time `t` (seconds since start).
pub fn state(t: f64) -> SimValues {
    // 40 s cycle: 0–6 idle, 6–30 accelerating through gears 1–5, 30–36 braking, 36–40 idle.
    let phase = t % 40.0;
    let (throttle, braking) = match phase {
        p if p < 6.0 => (0.0, false),
        p if p < 30.0 => (35.0 + 60.0 * ((p - 6.0) * 0.9).sin().abs(), false),
        p if p < 36.0 => (0.0, true),
        _ => (0.0, false),
    };
    let accel = (6.0..30.0).contains(&phase);
    let gear = if accel {
        1 + ((phase - 6.0) / 4.8) as u8
    } else if braking {
        3
    } else {
        0
    };
    let within_gear = if accel {
        ((phase - 6.0) % 4.8) / 4.8
    } else {
        0.0
    };
    let rpm = if accel {
        2800.0 + 4300.0 * within_gear
    } else if braking {
        4200.0 - 2800.0 * ((phase - 30.0) / 6.0)
    } else {
        850.0 + 25.0 * (t * 3.1).sin()
    };
    let speed = if accel {
        12.0 + (phase - 6.0) * 6.5
    } else if braking {
        168.0 * (1.0 - (phase - 30.0) / 6.0)
    } else {
        0.0
    };
    let coolant = (20.0 + t * 1.5).min(88.0 + 2.0 * (t * 0.05).sin());
    let map = if braking {
        25.0
    } else {
        32.0 + throttle * 0.75
    };
    let target = if throttle > 70.0 { 12.5 } else { 14.7 };
    let afr = if braking {
        18.0
    } else {
        target + 0.3 * (t * 7.0).sin()
    };
    let mut engine = ENGINE_RUN;
    if coolant < 60.0 {
        engine |= ENGINE_WARMUP;
    }
    if accel && within_gear < 0.1 {
        engine |= ENGINE_TPS_ACCEL;
    }
    let mut status2 = STATUS2_SYNC;
    if rpm > 7000.0 {
        status2 |= STATUS2_SOFT_LIMIT;
    }
    SimValues {
        secl: (t as u64 % 256) as u8,
        engine,
        status2,
        map_kpa: map as u16,
        iat_c: 28 + (throttle / 20.0) as i16,
        coolant_c: coolant as i16,
        battery_v: 13.9 + 0.2 * (t * 0.7).sin() as f32,
        afr: afr as f32,
        rpm: rpm as u16,
        ve: (55.0 + throttle * 0.4) as u8,
        afr_target: target as f32,
        advance: (12.0 + (rpm / 7100.0) * 22.0 - throttle * 0.05) as i8,
        tps: throttle as u8,
        baro: 101,
        pw1_ms: if braking {
            0.0
        } else {
            (1.6 + map * 0.08) as f32
        },
        dwell_ms: 3.2,
        loops_per_second: 2400,
        vss_kmh: speed as u16,
        gear,
        oil_psi: (20.0 + rpm * 0.007).min(80.0) as u8,
        fuel_psi: 43,
    }
}

pub async fn run(config: Arc<AppConfig>, tx: Tx, cancel: CancellationToken) {
    let start = Instant::now();
    let _ = tx
        .send(SourceEvent::Identified(
            SIGNATURE.into(),
            EcuProtocol::Speeduino,
        ))
        .await;
    send_link(&tx, LinkStatus::Connected, "simulator");
    let mut interval = tokio::time::interval(Duration::from_millis(config.refresh_rate_ms));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = cancel.cancelled() => return,
            _ = interval.tick() => {}
        }
        let packet = encode(&state(start.elapsed().as_secs_f64()));
        match poller::decode(EcuProtocol::Speeduino, &packet, now_ms()) {
            Ok(sample) => {
                if tx
                    .send(SourceEvent::Sample(Box::new(sample)))
                    .await
                    .is_err()
                {
                    return;
                }
            }
            Err(e) => tracing::error!("simulator produced an undecodable packet: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::speeduino::{flags, parse};

    #[test]
    fn model_covers_idle_acceleration_and_braking() {
        let idle = state(2.0);
        assert!((800..900).contains(&idle.rpm) && idle.tps == 0);
        let pull = state(20.0);
        assert!(pull.rpm > 2800 && pull.tps > 30 && pull.gear >= 3);
        let brake = state(33.0);
        assert_eq!(brake.pw1_ms, 0.0, "fuel cut on overrun");
        // Coolant warms up and stays plausible.
        assert!(state(0.0).coolant_c < 30 && (85..=90).contains(&state(600.0).coolant_c));
    }

    #[test]
    fn every_state_decodes_with_sync_and_running_bits() {
        for t in (0..200).map(|i| i as f64 * 0.37) {
            let d = parse(&encode(&state(t))).unwrap();
            let on: Vec<_> = flags(&d)
                .into_iter()
                .filter(|f| f.on)
                .map(|f| f.name)
                .collect();
            assert!(on.contains(&"running") && on.contains(&"sync"), "t={t}");
            let c = crate::protocol::speeduino::canonical(&d);
            assert!(c.contains_key("rpm") && c.contains_key("afr"), "t={t}");
        }
    }
}
