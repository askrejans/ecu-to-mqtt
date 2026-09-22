//! Manufacturer CAN profiles. Decode only received groups: a slow temperature
//! packet must not refresh a stopped RPM stream. See ECU_PROTOCOLS.md for sources.
use crate::ecu_protocol::{CanInputFrame, Channels, EcuProtocol, add, signed, unsigned};
use crate::errors::Result;

fn le(data: &[u8], offset: usize) -> f64 {
    u16::from_le_bytes([data[offset], data[offset + 1]]) as f64
}
fn sle(data: &[u8], offset: usize) -> f64 {
    i16::from_le_bytes([data[offset], data[offset + 1]]) as f64
}

pub fn decode(protocol: EcuProtocol, base: u32, frame: &CanInputFrame) -> Result<Channels> {
    let mut out = Channels::new();
    let Some(group) = frame.id.checked_sub(base) else {
        return Ok(out);
    };
    let d = &frame.data;
    match protocol {
        EcuProtocol::HaltechCanV2 => match group {
            0 => {
                add(&mut out, "rpm", unsigned(d, 0));
                add(&mut out, "manifoldKpa", unsigned(d, 2) / 10.);
                add(&mut out, "throttle", unsigned(d, 4) / 10.);
            }
            1 => {
                add(&mut out, "fuelPressureKpa", unsigned(d, 0) / 10. - 101.3);
                add(&mut out, "oilPressureKpa", unsigned(d, 2) / 10. - 101.3);
            }
            2 => add(&mut out, "ignitionDeg", signed(d, 4) / 10.),
            8 => {
                add(&mut out, "lambda", unsigned(d, 0) / 1000.);
                add(&mut out, "lambda2", unsigned(d, 2) / 1000.);
            }
            0xb => {
                add(&mut out, "brakePressureKpa", unsigned(d, 0) - 101.3);
                add(&mut out, "lateralG", signed(d, 6) / 98.0665);
            }
            0xe => add(&mut out, "longitudinalG", signed(d, 6) / 98.0665),
            0x10 => {
                add(&mut out, "ecuSpeedKmh", unsigned(d, 0) / 10.);
                // Low byte is gear; high byte is a separate selector enum.
                add(&mut out, "gear", d[3] as i8 as f64);
            }
            0x12 => add(&mut out, "batteryV", unsigned(d, 0) / 10.),
            0x80 => {
                for (offset, name) in [(0, "coolantC"), (2, "intakeC"), (4, "fuelC"), (6, "oilC")] {
                    add(&mut out, name, unsigned(d, offset) / 10. - 273.15);
                }
            }
            _ => {}
        },
        EcuProtocol::MaxxecuCanV12 | EcuProtocol::MaxxecuCanV13 => match group {
            0 => {
                add(&mut out, "rpm", sle(d, 0));
                add(&mut out, "throttle", sle(d, 2) / 10.);
                add(&mut out, "manifoldKpa", sle(d, 4) / 10.);
                add(&mut out, "lambda", sle(d, 6) / 1000.);
            }
            1 => add(&mut out, "ignitionDeg", sle(d, 4) / 10.),
            2 => add(&mut out, "ecuSpeedKmh", sle(d, 6) / 10.),
            0x10 => {
                add(&mut out, "batteryV", sle(d, 0) / 100.);
                add(&mut out, "intakeC", sle(d, 4) / 10.);
                add(&mut out, "coolantC", sle(d, 6) / 10.);
            }
            0x16 => {
                add(&mut out, "gear", sle(d, 0));
                if protocol == EcuProtocol::MaxxecuCanV13 {
                    add(&mut out, "oilPressureKpa", sle(d, 4) / 10.);
                    add(&mut out, "oilC", sle(d, 6) / 10.);
                }
            }
            0x17 if protocol == EcuProtocol::MaxxecuCanV13 => {
                add(&mut out, "fuelPressureKpa", sle(d, 0) / 10.)
            }
            6 if protocol == EcuProtocol::MaxxecuCanV13 => {
                add(&mut out, "brakeSwitch", (d[1] & 1) as f64);
                add(&mut out, "clutchSwitch", ((d[1] >> 1) & 1) as f64);
            }
            7 if protocol == EcuProtocol::MaxxecuCanV13 => {
                add(&mut out, "longitudinalG", sle(d, 0) / 100.);
                add(&mut out, "lateralG", sle(d, 2) / 100.);
            }
            _ => {}
        },
        EcuProtocol::EcumasterEmuCan => match group {
            0 => {
                add(&mut out, "rpm", le(d, 0));
                add(&mut out, "throttle", d[2] as f64 / 2.);
                add(&mut out, "intakeC", d[3] as i8 as f64);
                add(&mut out, "manifoldKpa", le(d, 4));
            }
            2 => {
                add(&mut out, "ecuSpeedKmh", le(d, 0));
                add(&mut out, "oilC", d[3] as f64);
                add(&mut out, "oilPressureKpa", d[4] as f64 * 6.25);
                add(&mut out, "fuelPressureKpa", d[5] as f64 * 6.25);
                add(&mut out, "coolantC", sle(d, 6));
            }
            3 => {
                add(&mut out, "ignitionDeg", d[0] as i8 as f64 / 2.);
                add(&mut out, "lambda", d[2] as f64 / 128.);
            }
            4 => {
                if d[0] <= 7 {
                    add(&mut out, "gear", d[0] as f64);
                }
                add(&mut out, "batteryV", le(d, 2) * 0.027);
            }
            _ => {}
        },
        EcuProtocol::AemnetCan => match group {
            0 => {
                add(&mut out, "rpm", unsigned(d, 0) * 0.39063);
                add(&mut out, "throttle", unsigned(d, 4) * 0.0015259);
                add(&mut out, "intakeC", d[6] as i8 as f64);
                add(&mut out, "coolantC", d[7] as i8 as f64);
            }
            3 => {
                add(&mut out, "lambda", d[0] as f64 / 256. + 0.5);
                add(&mut out, "lambda2", d[1] as f64 / 256. + 0.5);
                add(&mut out, "ecuSpeedKmh", unsigned(d, 2) * 0.0062865);
                add(&mut out, "gear", d[4] as f64);
                add(&mut out, "ignitionDeg", d[5] as f64 * 0.35156 - 17.);
                add(&mut out, "batteryV", unsigned(d, 6) * 0.0002455);
            }
            4 => {
                add(&mut out, "manifoldKpa", unsigned(d, 0) / 10.);
                add(
                    &mut out,
                    "fuelPressureKpa",
                    d[3] as f64 * 0.580151 * 6.894757,
                );
                add(
                    &mut out,
                    "oilPressureKpa",
                    d[4] as f64 * 0.580151 * 6.894757,
                );
            }
            7 => add(&mut out, "oilC", d[6] as f64 - 50.),
            _ => {}
        },
        EcuProtocol::LinkGenericDash if group == 0 && d[1] == 0 => match d[0] {
            0 => {
                add(&mut out, "rpm", le(d, 2));
                add(&mut out, "manifoldKpa", le(d, 4));
            }
            1 => add(&mut out, "throttle", le(d, 4) / 10.),
            2 => add(&mut out, "coolantC", le(d, 6) - 50.),
            3 => {
                add(&mut out, "intakeC", le(d, 2) - 50.);
                add(&mut out, "batteryV", le(d, 4) / 100.);
            }
            4 => {
                let gear = le(d, 2);
                if gear <= 8. {
                    add(&mut out, "gear", gear);
                }
                add(&mut out, "ignitionDeg", le(d, 6) / 10. - 100.);
            }
            6 => {
                add(&mut out, "lambda", le(d, 4) / 1000.);
                add(&mut out, "lambda2", le(d, 6) / 1000.);
            }
            7 => add(&mut out, "fuelPressureKpa", le(d, 6)),
            8 => {
                add(&mut out, "oilC", le(d, 2) - 50.);
                add(&mut out, "oilPressureKpa", le(d, 4));
                add(&mut out, "wheelSpeedFlKmh", le(d, 6) / 10.);
            }
            9 => {
                add(&mut out, "wheelSpeedRlKmh", le(d, 2) / 10.);
                add(&mut out, "wheelSpeedFrKmh", le(d, 4) / 10.);
                add(&mut out, "wheelSpeedRrKmh", le(d, 6) / 10.);
            }
            _ => {}
        },
        EcuProtocol::LinkGenericDash2 => match group {
            0 => {
                add(&mut out, "rpm", unsigned(d, 0));
                // MGP is gauge pressure; do not silently label it absolute MAP.
                add(&mut out, "boostKpa", unsigned(d, 2) - 100.);
                add(&mut out, "coolantC", d[4] as f64 - 50.);
                add(&mut out, "intakeC", d[5] as f64 - 50.);
                add(&mut out, "batteryV", d[6] as f64 / 10.);
                add(&mut out, "oilC", d[7] as f64 - 50.);
            }
            1 => {
                add(&mut out, "throttle", unsigned(d, 0) / 10.);
                add(&mut out, "ignitionDeg", unsigned(d, 2) / 10. - 100.);
                add(&mut out, "ecuSpeedKmh", d[4] as f64);
                add(&mut out, "oilPressureKpa", d[5] as f64 * 10.);
                add(&mut out, "fuelPressureKpa", d[6] as f64 * 10.);
            }
            2 => {
                add(&mut out, "lambda", unsigned(d, 0) / 1000.);
                add(&mut out, "lambda2", unsigned(d, 2) / 1000.);
            }
            3 if d[0] <= 6 => add(&mut out, "gear", d[0] as f64),
            _ => {}
        },
        EcuProtocol::MotecM1Pdm => match group {
            0 => {
                add(&mut out, "rpm", d[0] as f64 * 100.);
                add(&mut out, "throttle", d[1] as f64);
                add(&mut out, "ecuSpeedKmh", d[2] as f64);
                for (offset, name) in [
                    (3, "coolantC"),
                    (4, "oilC"),
                    (5, "fuelC"),
                    (6, "transmissionC"),
                    (7, "differentialC"),
                ] {
                    add(&mut out, name, d[offset] as f64);
                }
            }
            1 => {
                add(&mut out, "brakeSwitch", ((d[4] >> 5) & 1) as f64);
                add(&mut out, "clutchSwitch", ((d[4] >> 3) & 1) as f64);
            }
            _ => {}
        },
        _ => {}
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ecu_protocol::decode_can;
    fn frame(protocol: EcuProtocol, offset: u32, data: [u8; 8]) -> Channels {
        decode_can(
            protocol,
            protocol.default_can_base(),
            &CanInputFrame {
                id: protocol.default_can_base() + offset,
                extended: protocol.extended(),
                data: data.to_vec(),
                timestamp_ms: None,
            },
        )
        .unwrap()
    }
    #[test]
    fn haltech_kelvin_gauge_pressure_and_signed_acceleration() {
        let core = frame(
            EcuProtocol::HaltechCanV2,
            0,
            [0x17, 0x70, 0x03, 0xf5, 0x02, 0xee, 0, 0],
        );
        assert_eq!(core["rpm"], 6000.);
        assert_eq!(core["manifoldKpa"], 101.3);
        assert_eq!(core["throttle"], 75.);
        let temp = frame(
            EcuProtocol::HaltechCanV2,
            0x80,
            [0x0e, 0x2f, 0x0b, 0xa6, 0, 0, 0, 0],
        );
        assert!((temp["coolantC"] - 89.95).abs() < 0.001);
        assert!((temp["intakeC"] - 25.05).abs() < 0.001);
        assert!(!temp.contains_key("rpm"));
        assert!(!temp.contains_key("oilC")); // Kelvin zero is invalid, not a plausible cold engine.
        let pressure = frame(
            EcuProtocol::HaltechCanV2,
            1,
            [0x13, 0x95, 0x0f, 0xad, 0, 0, 0, 0],
        );
        assert!((pressure["fuelPressureKpa"] - 400.).abs() < 0.001);
        let acceleration = frame(
            EcuProtocol::HaltechCanV2,
            0xe,
            [0, 0, 0, 0, 0, 0, 0xff, 0x9e],
        );
        assert!((acceleration["longitudinalG"] + 0.999322).abs() < 0.001);
    }
    #[test]
    fn maxxecu_versions_do_not_decode_reserved_oil_fields() {
        let core = frame(
            EcuProtocol::MaxxecuCanV13,
            0,
            [0x70, 0x17, 0xee, 2, 0xf5, 3, 0x84, 3],
        );
        assert_eq!(core["rpm"], 6000.);
        assert_eq!(core["lambda"], 0.9);
        assert!(!core.contains_key("afr")); // Fuel stoichiometry is not assumed.
        let old = frame(
            EcuProtocol::MaxxecuCanV12,
            0x16,
            [4, 0, 0, 0, 0x70, 0x17, 0xe8, 3],
        );
        let new = frame(
            EcuProtocol::MaxxecuCanV13,
            0x16,
            [4, 0, 0, 0, 0x70, 0x17, 0xe8, 3],
        );
        assert_eq!(old.len(), 1);
        assert_eq!(new["oilPressureKpa"], 600.);
        assert_eq!(new["oilC"], 100.);
        let cold = frame(
            EcuProtocol::MaxxecuCanV13,
            0x10,
            [0x64, 5, 0, 0, 0x9c, 0xff, 0x84, 3],
        );
        assert_eq!(cold["intakeC"], -10.);
        assert_eq!(cold["batteryV"], 13.8);
    }
    #[test]
    fn ecumaster_mixed_width_signed_fields_and_bar_to_kpa() {
        let core = frame(
            EcuProtocol::EcumasterEmuCan,
            0,
            [0x70, 0x17, 150, 246, 101, 0, 0, 0],
        );
        assert_eq!(core["rpm"], 6000.);
        assert_eq!(core["throttle"], 75.);
        assert_eq!(core["intakeC"], -10.);
        let secondary = frame(
            EcuProtocol::EcumasterEmuCan,
            2,
            [120, 0, 100, 105, 64, 80, 90, 0],
        );
        assert_eq!(secondary["oilPressureKpa"], 400.);
        assert_eq!(secondary["fuelPressureKpa"], 500.);
        let battery = frame(EcuProtocol::EcumasterEmuCan, 4, [255, 0, 0, 2, 0, 0, 0, 0]);
        assert!((battery["batteryV"] - 13.824).abs() < 0.0001);
        assert!(!battery.contains_key("gear"));
    }
    #[test]
    fn aem_requires_extended_identifier_and_preserves_lambda() {
        let core = frame(EcuProtocol::AemnetCan, 0, [0x3c, 0, 0, 0, 0x80, 0, 246, 90]);
        assert!((core["rpm"] - 6000.).abs() < 0.1);
        assert!((core["throttle"] - 50.).abs() < 0.01);
        assert_eq!(core["intakeC"], -10.);
        let second = frame(
            EcuProtocol::AemnetCan,
            3,
            [128, 128, 0, 0, 4, 100, 0xdb, 0x93],
        );
        assert_eq!(second["lambda"], 1.);
        assert!((second["batteryV"] - 13.8).abs() < 0.01);
        assert!(
            decode_can(
                EcuProtocol::AemnetCan,
                0x01f0a000,
                &CanInputFrame {
                    id: 0x01f0a000,
                    extended: false,
                    data: vec![0; 8],
                    timestamp_ms: None,
                }
            )
            .is_err()
        );
    }
    #[test]
    fn link_compound_discriminator_and_motec_low_resolution() {
        let core = frame(
            EcuProtocol::LinkGenericDash,
            0,
            [0, 0, 0x70, 0x17, 101, 0, 0, 0],
        );
        assert_eq!(core["rpm"], 6000.);
        assert_eq!(core["manifoldKpa"], 101.);
        assert!(frame(EcuProtocol::LinkGenericDash, 0, [1, 1, 0, 0, 0, 0, 0, 0]).is_empty());
        let m1 = frame(
            EcuProtocol::MotecM1Pdm,
            0,
            [60, 75, 120, 90, 100, 30, 80, 85],
        );
        assert_eq!(m1["rpm"], 6000.);
        assert_eq!(m1["coolantC"], 90.);
        assert!(!m1.contains_key("batteryV"));
        assert!(!m1.contains_key("brake"));
    }
    #[test]
    fn link_generic_scales_match_pclink_782_help_not_other_dash_formats() {
        let temp = frame(
            EcuProtocol::LinkGenericDash,
            0,
            [3, 0, 40, 0, 0x64, 5, 0, 0],
        );
        assert_eq!(temp["intakeC"], -10.);
        assert_eq!(temp["batteryV"], 13.8);
        assert!(!temp.contains_key("rpm"));
        let coolant = frame(EcuProtocol::LinkGenericDash, 0, [2, 0, 0, 0, 0, 0, 140, 0]);
        assert_eq!(coolant["coolantC"], 90.);
        let dash2 = frame(
            EcuProtocol::LinkGenericDash2,
            0,
            [0x17, 0x70, 0, 150, 140, 40, 138, 155],
        );
        assert_eq!(dash2["rpm"], 6000.);
        assert_eq!(dash2["boostKpa"], 50.);
        assert!(!dash2.contains_key("manifoldKpa"));
        assert_eq!(dash2["coolantC"], 90.);
        assert_eq!(dash2["batteryV"], 13.8);
    }

    #[test]
    fn malformed_unrelated_and_wrong_format_frames_are_not_telemetry() {
        for protocol in [
            EcuProtocol::HaltechCanV2,
            EcuProtocol::MaxxecuCanV13,
            EcuProtocol::EcumasterEmuCan,
            EcuProtocol::AemnetCan,
            EcuProtocol::LinkGenericDash,
            EcuProtocol::MotecM1Pdm,
        ] {
            let base = protocol.default_can_base();
            for length in [0, 1, 7, 9, 64] {
                assert!(
                    decode_can(
                        protocol,
                        base,
                        &CanInputFrame {
                            id: base,
                            extended: protocol.extended(),
                            data: vec![0; length],
                            timestamp_ms: None,
                        }
                    )
                    .is_err()
                );
            }
            assert!(
                decode_can(
                    protocol,
                    base,
                    &CanInputFrame {
                        id: base - 1,
                        extended: protocol.extended(),
                        data: vec![0; 8],
                        timestamp_ms: None,
                    }
                )
                .unwrap()
                .is_empty()
            );
        }
    }
}
