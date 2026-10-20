//! Speeduino primary-serial `A` realtime packet.
//!
//! Byte layout from `getTSLogEntry()` in `speeduino/logger.cpp`. Firmware
//! 2025.01 (`speeduino 202501`) sends **130 bytes** (cases 0–129); some older
//! or modified builds append PW5–PW8 for 138 bytes, which are decoded when present.
//! The secondary-serial `A` reply (75 bytes) has a different layout and is rejected.
//!
//! All multi-byte values are little-endian. Temperatures carry a +40 offset.

use super::{Channels, add};
use crate::errors::{DecodeError, Result};
use serde::Serialize;

/// Bytes in a 2025.01 primary-serial `A` packet.
pub const PACKET_LEN: usize = 130;

// ---------------------------------------------------------------------------
// Data structure
// ---------------------------------------------------------------------------

/// Parsed Speeduino realtime data from the 'A' command response.
///
/// Field names, byte offsets, and units follow the wiki protocol specification.
/// Multi-byte values are pre-combined and stored in their natural Rust types.
#[derive(Debug, Clone, Default, Serialize)]
pub struct SpeeduinoData {
    /// Byte 0 – seconds counter (resets to 0 on ECU reset)
    pub secl: u8,
    /// Byte 1 – status1 bitfield
    pub status1: u8,
    /// Byte 2 – engine status bitfield
    pub engine: u8,
    /// Byte 3 – sync-loss counter
    pub sync_loss_counter: u8,

    /// Bytes 4–5 – MAP sensor (kPa, little-endian u16)
    pub map: u16,
    /// Byte 6 – IAT raw (stored as `actual_°C + 40`)
    pub iat_raw: u8,
    /// Byte 7 – coolant raw (stored as `actual_°C + 40`)
    pub coolant_raw: u8,
    /// Byte 8 – battery voltage correction (%)
    pub bat_correction: u8,
    /// Byte 9 – battery voltage × 10  (e.g. 142 = 14.2 V)
    pub battery_10: u8,
    /// Byte 10 – primary O2 sensor
    pub o2_primary: u8,
    /// Byte 11 – EGO correction (%)
    pub ego_correction: u8,
    /// Byte 12 – IAT fuel correction (%)
    pub iat_correction: u8,
    /// Byte 13 – warm-up enrichment correction (%)
    pub wue_correction: u8,

    /// Bytes 14–15 – engine speed (RPM, little-endian u16)
    pub rpm: u16,
    /// Byte 16 – TAE / accel-enrichment amount (stored >> 1; × 2 for actual %)
    pub tae_amount_raw: u8,
    /// Bytes 17–18 – total fuel/ignition corrections (gamma-E, little-endian u16, %)
    pub corrections: u16,

    /// Byte 19 – VE table 1 (%)
    pub ve1: u8,
    /// Byte 20 – VE table 2 (%)
    pub ve2: u8,
    /// Byte 21 – AFR target (stored × 10, e.g. 147 = 14.7)
    pub afr_target: u8,
    /// Bytes 22–23 – TPS rate of change (% × 10 per 100 ms, little-endian u16)
    pub tps_dot: u16,
    /// Byte 24 – ignition advance (degrees BTDC)
    pub advance: i8,
    /// Byte 25 – throttle position (0–100 %)
    pub tps: u8,

    /// Bytes 26–27 – MCU loops per second (little-endian u16)
    pub loops_per_second: u16,
    /// Bytes 28–29 – free RAM bytes (little-endian u16)
    pub free_ram: u16,

    /// Byte 30 – boost target (stored >> 1; × 2 for actual kPa)
    pub boost_target_raw: u8,
    /// Byte 31 – boost solenoid duty (stored / 100; × 100 for actual %)
    pub boost_duty_raw: u8,
    /// Byte 32 – `status2` bitfield (launch, rev limiters, boost cut, sync)
    pub status2: u8,

    /// Bytes 33–34 – RPM rate of change (little-endian i16)
    pub rpm_dot: i16,
    /// Byte 35 – ethanol / flex-fuel percentage (0–100 %)
    pub ethanol_pct: u8,
    /// Byte 36 – flex-fuel fuel correction (%)
    pub flex_correction: u8,
    /// Byte 37 – flex-fuel ignition correction (degrees)
    pub flex_ign_correction: u8,
    /// Byte 38 – idle load
    pub idle_load: u8,
    /// Byte 39 – test outputs bitfield
    pub test_outputs: u8,
    /// Byte 40 – secondary O2 sensor
    pub o2_secondary: u8,
    /// Byte 41 – barometric pressure (kPa)
    pub baro: u8,

    /// Bytes 42–73 – CAN inputs (16 channels, each a little-endian u16)
    pub canin: [u16; 16],

    /// Byte 74 – TPS ADC raw value
    pub tps_adc: u8,
    /// Byte 75 – next error code
    pub next_error: u8,

    /// Bytes 76–77 – injector pulse width 1 (µs / 10, little-endian u16)
    pub pw1: u16,
    /// Bytes 78–79 – injector pulse width 2
    pub pw2: u16,
    /// Bytes 80–81 – injector pulse width 3
    pub pw3: u16,
    /// Bytes 82–83 – injector pulse width 4
    pub pw4: u16,

    /// Byte 84 – status3 bitfield
    pub status3: u8,
    /// Byte 85 – engine protection status
    pub engine_protect_status: u8,

    /// Bytes 86–87 – fuel load (little-endian u16)
    pub fuel_load: u16,
    /// Bytes 88–89 – ignition load (little-endian u16)
    pub ign_load: u16,
    /// Bytes 90–91 – dwell time (0.1 ms units, little-endian u16)
    pub dwell: u16,

    /// Byte 92 – closed-loop idle target
    pub cl_idle_target: u8,
    /// Bytes 93–94 – MAP rate of change (kPa × 10 per 100 ms, little-endian u16)
    pub map_dot: u16,

    /// Bytes 95–96 – VVT1 cam angle (little-endian i16, degrees)
    pub vvt1_angle: i16,
    /// Byte 97 – VVT1 target angle (degrees)
    pub vvt1_target_angle: u8,
    /// Byte 98 – VVT1 solenoid duty (%)
    pub vvt1_duty: u8,

    /// Bytes 99–100 – flex-fuel boost correction (little-endian u16)
    pub flex_boost_correction: u16,
    /// Byte 101 – barometric pressure correction (%)
    pub baro_correction: u8,
    /// Byte 102 – current effective VE (blended from VE1/VE2, %)
    pub ve_current: u8,
    /// Byte 103 – after-start enrichment value (%)
    pub ase_value: u8,

    /// Bytes 104–105 – vehicle speed (km/h, little-endian u16)
    pub vss: u16,
    /// Byte 106 – current gear
    pub gear: u8,
    /// Byte 107 – fuel pressure (kPa)
    pub fuel_pressure: u8,
    /// Byte 108 – oil pressure (kPa)
    pub oil_pressure: u8,
    /// Byte 109 – water-methanol injection pulse width
    pub wmi_pw: u8,
    /// Byte 110 – status4 bitfield
    pub status4: u8,

    /// Bytes 111–112 – VVT2 cam angle (little-endian i16, degrees)
    pub vvt2_angle: i16,
    /// Byte 113 – VVT2 target angle (degrees)
    pub vvt2_target_angle: u8,
    /// Byte 114 – VVT2 solenoid duty (%)
    pub vvt2_duty: u8,

    /// Byte 115 – outputs status bitfield
    pub outputs_status: u8,
    /// Byte 116 – fuel temperature raw (stored as `actual_°C + 40`)
    pub fuel_temp_raw: u8,
    /// Byte 117 – fuel temperature correction (%)
    pub fuel_temp_correction: u8,
    /// Byte 118 – advance table 1 (degrees)
    pub advance1: i8,
    /// Byte 119 – advance table 2 (degrees)
    pub advance2: i8,
    /// Byte 120 – TunerStudio SD card status
    pub ts_sd_status: u8,

    /// Bytes 121–122 – EMAP sensor (kPa, little-endian u16).
    /// Present when packet ≥ 123 bytes (always in valid 130-byte primary-serial packets).
    pub emap: Option<u16>,

    // ---- Extended fields (bytes 123–129): present in all 130-byte packets ----
    /// Byte 123 – radiator fan duty cycle (%)
    pub fan_duty: Option<u8>,
    /// Byte 124 – air conditioning status bitfield
    pub air_con_status: Option<u8>,
    /// Bytes 125–126 – actual (measured) dwell time (0.1 ms units, little-endian u16)
    pub actual_dwell: Option<u16>,
    /// Byte 127 – status5 bitfield
    pub status5: Option<u8>,
    /// Byte 128 – knock event counter
    pub knock_count: Option<u8>,
    /// Byte 129 – knock retard (degrees)
    pub knock_retard: Option<u8>,

    // ---- PW5–PW8 (bytes 130–137): only in 138-byte packets of other builds ----
    /// Bytes 130–131 – injector pulse width 5 (µs / 10, little-endian u16)
    pub pw5: Option<u16>,
    /// Bytes 132–133 – injector pulse width 6
    pub pw6: Option<u16>,
    /// Bytes 134–135 – injector pulse width 7
    pub pw7: Option<u16>,
    /// Bytes 136–137 – injector pulse width 8
    pub pw8: Option<u16>,
}

impl SpeeduinoData {
    pub fn iat_celsius(&self) -> i16 {
        self.iat_raw as i16 - 40
    }
    pub fn coolant_celsius(&self) -> i16 {
        self.coolant_raw as i16 - 40
    }
    pub fn fuel_temp_celsius(&self) -> i16 {
        self.fuel_temp_raw as i16 - 40
    }
    pub fn battery_voltage(&self) -> f32 {
        self.battery_10 as f32 / 10.0
    }
    pub fn afr_target_real(&self) -> f32 {
        self.afr_target as f32 / 10.0
    }
    pub fn tae_amount_pct(&self) -> u16 {
        self.tae_amount_raw as u16 * 2
    }
    pub fn boost_target_kpa(&self) -> u16 {
        self.boost_target_raw as u16 * 2
    }
    pub fn boost_duty_pct(&self) -> u32 {
        self.boost_duty_raw as u32 * 100
    }
    pub fn pw1_ms(&self) -> f32 {
        self.pw1 as f32 / 10.0
    }
    pub fn pw2_ms(&self) -> f32 {
        self.pw2 as f32 / 10.0
    }
    pub fn pw3_ms(&self) -> f32 {
        self.pw3 as f32 / 10.0
    }
    pub fn pw4_ms(&self) -> f32 {
        self.pw4 as f32 / 10.0
    }
    pub fn dwell_ms(&self) -> f32 {
        self.dwell as f32 / 10.0
    }
}

// ---------------------------------------------------------------------------
// Internal parser
// ---------------------------------------------------------------------------

/// Parse a primary-serial `A` packet (≥ 130 bytes).
pub fn parse(data: &[u8]) -> Result<SpeeduinoData> {
    // Byte layout matches `getTSLogEntry()` in Speeduino logger.cpp.
    // Minimum 130 bytes (firmware 2025.01); longer packets are accepted.
    // The secondary-serial 'A' response (75 bytes) uses a different, incompatible layout.
    const MIN_BYTES: usize = 130;
    if data.len() < MIN_BYTES {
        return Err(DecodeError::Short {
            expected: MIN_BYTES,
            actual: data.len(),
        });
    }

    // Inline helpers for little-endian multi-byte reads.
    let u16_le = |lo: usize, hi: usize| -> u16 { (data[hi] as u16) << 8 | data[lo] as u16 };
    let i16_le = |lo: usize, hi: usize| -> i16 { u16_le(lo, hi) as i16 };

    // Bytes 42–73: 16 CAN input channels (2 bytes each, little-endian u16)
    let mut canin = [0u16; 16];
    for (channel, value) in canin.iter_mut().enumerate() {
        *value = u16_le(42 + channel * 2, 43 + channel * 2);
    }

    // Optional fields — all present in 130-byte packets.
    // PW5–PW8 (bytes 130–137) need a 138-byte packet.
    let emap = if data.len() >= 123 {
        Some(u16_le(121, 122))
    } else {
        None
    };
    let fan_duty = if data.len() >= 124 {
        Some(data[123])
    } else {
        None
    };
    let air_con_status = if data.len() >= 125 {
        Some(data[124])
    } else {
        None
    };
    let actual_dwell = if data.len() >= 127 {
        Some(u16_le(125, 126))
    } else {
        None
    };
    let status5 = if data.len() >= 128 {
        Some(data[127])
    } else {
        None
    };
    let knock_count = if data.len() >= 129 {
        Some(data[128])
    } else {
        None
    };
    let knock_retard = if data.len() >= 130 {
        Some(data[129])
    } else {
        None
    };
    let pw5 = if data.len() >= 132 {
        Some(u16_le(130, 131))
    } else {
        None
    };
    let pw6 = if data.len() >= 134 {
        Some(u16_le(132, 133))
    } else {
        None
    };
    let pw7 = if data.len() >= 136 {
        Some(u16_le(134, 135))
    } else {
        None
    };
    let pw8 = if data.len() >= 138 {
        Some(u16_le(136, 137))
    } else {
        None
    };

    let parsed = SpeeduinoData {
        // ---- Bytes 0–3 ----------------------------------------
        secl: data[0],
        status1: data[1],
        engine: data[2],
        sync_loss_counter: data[3],
        // ---- Bytes 4–21 ---------------------------------------
        map: u16_le(4, 5),
        iat_raw: data[6],
        coolant_raw: data[7],
        bat_correction: data[8],
        battery_10: data[9],
        o2_primary: data[10],
        ego_correction: data[11],
        iat_correction: data[12],
        wue_correction: data[13],
        rpm: u16_le(14, 15),
        tae_amount_raw: data[16],
        corrections: u16_le(17, 18),
        ve1: data[19],
        ve2: data[20],
        afr_target: data[21],
        // ---- Bytes 22–41 — NOTE: tpsDOT is u16 (two bytes) ----
        tps_dot: u16_le(22, 23),
        advance: data[24] as i8,
        tps: data[25],
        loops_per_second: u16_le(26, 27),
        free_ram: u16_le(28, 29),
        boost_target_raw: data[30],
        boost_duty_raw: data[31],
        status2: data[32],
        rpm_dot: i16_le(33, 34),
        ethanol_pct: data[35],
        flex_correction: data[36],
        flex_ign_correction: data[37],
        idle_load: data[38],
        test_outputs: data[39],
        o2_secondary: data[40],
        baro: data[41],
        // ---- Bytes 42–73 (CAN inputs) -------------------------
        canin,
        // ---- Bytes 74–91 --------------------------------------
        tps_adc: data[74],
        next_error: data[75],
        pw1: u16_le(76, 77),
        pw2: u16_le(78, 79),
        pw3: u16_le(80, 81),
        pw4: u16_le(82, 83),
        status3: data[84],
        engine_protect_status: data[85],
        fuel_load: u16_le(86, 87),
        ign_load: u16_le(88, 89),
        dwell: u16_le(90, 91),
        // ---- Bytes 92–120 — NOTE: mapDOT is u16 (two bytes) ---
        cl_idle_target: data[92],
        map_dot: u16_le(93, 94),
        vvt1_angle: i16_le(95, 96),
        vvt1_target_angle: data[97],
        vvt1_duty: data[98],
        flex_boost_correction: u16_le(99, 100),
        baro_correction: data[101],
        ve_current: data[102],
        ase_value: data[103],
        vss: u16_le(104, 105),
        gear: data[106],
        fuel_pressure: data[107],
        oil_pressure: data[108],
        wmi_pw: data[109],
        status4: data[110],
        vvt2_angle: i16_le(111, 112),
        vvt2_target_angle: data[113],
        vvt2_duty: data[114],
        outputs_status: data[115],
        fuel_temp_raw: data[116],
        fuel_temp_correction: data[117],
        advance1: data[118] as i8,
        advance2: data[119] as i8,
        ts_sd_status: data[120],
        // ---- Optional / extended fields -----------------------
        emap,
        fan_duty,
        air_con_status,
        actual_dwell,
        status5,
        knock_count,
        knock_retard,
        pw5,
        pw6,
        pw7,
        pw8,
    };

    Ok(parsed)
}

// ---------------------------------------------------------------------------
// Legacy scalar topics
// ---------------------------------------------------------------------------

/// Build the full list of (topic-code, value) pairs for publishing.
/// Every Speeduino 'A' command parameter is present.
pub fn get_params_to_publish(d: &SpeeduinoData) -> Vec<(&'static str, String)> {
    let mut params: Vec<(&'static str, String)> = vec![
        // Engine basics
        ("RPM", d.rpm.to_string()),
        ("TPS", d.tps.to_string()),
        ("MAP", d.map.to_string()),
        ("BAR", d.baro.to_string()),
        ("BAT", format!("{:.1}", d.battery_voltage())),
        ("SCL", d.secl.to_string()),
        ("SYN", d.sync_loss_counter.to_string()),
        // Temperatures – raw bytes (backward-compatible names)
        ("MAT", d.iat_raw.to_string()),
        ("CAD", d.coolant_raw.to_string()),
        // Temperatures – converted to °C
        ("IAT", d.iat_celsius().to_string()),
        ("CLT", d.coolant_celsius().to_string()),
        // O2 sensors
        ("O2P", d.o2_primary.to_string()),
        ("O2S", d.o2_secondary.to_string()),
        // Fuel
        ("AFT", format!("{:.1}", d.afr_target_real())),
        ("VE1", d.ve1.to_string()),
        ("VE2", d.ve2.to_string()),
        ("VEC", d.ve_current.to_string()),
        ("PW1", format!("{:.1}", d.pw1_ms())),
        ("PW2", format!("{:.1}", d.pw2_ms())),
        ("PW3", format!("{:.1}", d.pw3_ms())),
        ("PW4", format!("{:.1}", d.pw4_ms())),
        // Ignition
        ("ADV", d.advance.to_string()),
        ("AD1", d.advance1.to_string()),
        ("AD2", d.advance2.to_string()),
        ("DWL", format!("{:.1}", d.dwell_ms())),
        // Legacy topic name kept; the byte is status2, not spark flags.
        ("SPK", d.status2.to_string()),
        // Corrections
        ("BTC", d.bat_correction.to_string()),
        ("EGC", d.ego_correction.to_string()),
        ("ITC", d.iat_correction.to_string()),
        ("WEC", d.wue_correction.to_string()),
        ("COR", d.corrections.to_string()),
        ("BRC", d.baro_correction.to_string()),
        ("ASE", d.ase_value.to_string()),
        ("TAE", d.tae_amount_pct().to_string()),
        // Boost
        ("BST", d.boost_target_kpa().to_string()),
        ("BSD", d.boost_duty_pct().to_string()),
        // Flex / ethanol
        ("ETH", d.ethanol_pct.to_string()),
        ("FLC", d.flex_correction.to_string()),
        ("FIC", d.flex_ign_correction.to_string()),
        ("FBC", d.flex_boost_correction.to_string()),
        // Fuel temperature
        ("FTP", d.fuel_temp_celsius().to_string()),
        ("FTC", d.fuel_temp_correction.to_string()),
        // Performance
        ("LPS", d.loops_per_second.to_string()),
        ("FRM", d.free_ram.to_string()),
        ("RPD", d.rpm_dot.to_string()),
        // Throttle detail
        ("TPD", d.tps_dot.to_string()),
        ("TAD", d.tps_adc.to_string()),
        // Load
        ("FLD", d.fuel_load.to_string()),
        ("IGD", d.ign_load.to_string()),
        // Idle
        ("ILL", d.idle_load.to_string()),
        ("MPD", d.map_dot.to_string()),
        ("CIT", d.cl_idle_target.to_string()),
        // VVT
        ("VA1", d.vvt1_angle.to_string()),
        ("VT1", d.vvt1_target_angle.to_string()),
        ("VD1", d.vvt1_duty.to_string()),
        ("VA2", d.vvt2_angle.to_string()),
        ("VT2", d.vvt2_target_angle.to_string()),
        ("VD2", d.vvt2_duty.to_string()),
        // Vehicle
        ("VSS", d.vss.to_string()),
        ("GER", d.gear.to_string()),
        // Pressures
        ("FPR", d.fuel_pressure.to_string()),
        ("OPR", d.oil_pressure.to_string()),
        // Misc
        ("WMI", d.wmi_pw.to_string()),
        ("TOF", d.test_outputs.to_string()),
        ("NER", d.next_error.to_string()),
        // Status bitfields
        ("STA", d.status1.to_string()),
        ("ENG", d.engine.to_string()),
        ("ST3", d.status3.to_string()),
        ("ST4", d.status4.to_string()),
        ("EPS", d.engine_protect_status.to_string()),
        ("OUT", d.outputs_status.to_string()),
        ("SDS", d.ts_sd_status.to_string()),
        // CAN inputs (CN01–CN16)
        ("CN01", d.canin[0].to_string()),
        ("CN02", d.canin[1].to_string()),
        ("CN03", d.canin[2].to_string()),
        ("CN04", d.canin[3].to_string()),
        ("CN05", d.canin[4].to_string()),
        ("CN06", d.canin[5].to_string()),
        ("CN07", d.canin[6].to_string()),
        ("CN08", d.canin[7].to_string()),
        ("CN09", d.canin[8].to_string()),
        ("CN10", d.canin[9].to_string()),
        ("CN11", d.canin[10].to_string()),
        ("CN12", d.canin[11].to_string()),
        ("CN13", d.canin[12].to_string()),
        ("CN14", d.canin[13].to_string()),
        ("CN15", d.canin[14].to_string()),
        ("CN16", d.canin[15].to_string()),
    ];

    // Optional EMAP and extended fields
    if let Some(v) = d.emap {
        params.push(("EMP", v.to_string()));
    }
    if let Some(v) = d.fan_duty {
        params.push(("FAN", v.to_string()));
    }
    if let Some(v) = d.air_con_status {
        params.push(("ACS", v.to_string()));
    }
    if let Some(v) = d.actual_dwell {
        params.push(("ADW", format!("{:.1}", v as f32 / 10.0)));
    }
    if let Some(v) = d.status5 {
        params.push(("ST5", v.to_string()));
    }
    if let Some(v) = d.knock_count {
        params.push(("KNC", v.to_string()));
    }
    if let Some(v) = d.knock_retard {
        params.push(("KNR", v.to_string()));
    }
    // PW5–PW8 — 138-byte packets only
    if let Some(v) = d.pw5 {
        params.push(("PW5", format!("{:.1}", v as f32 / 10.0)));
    }
    if let Some(v) = d.pw6 {
        params.push(("PW6", format!("{:.1}", v as f32 / 10.0)));
    }
    if let Some(v) = d.pw7 {
        params.push(("PW7", format!("{:.1}", v as f32 / 10.0)));
    }
    if let Some(v) = d.pw8 {
        params.push(("PW8", format!("{:.1}", v as f32 / 10.0)));
    }

    params
}

// ---------------------------------------------------------------------------
// Canonical channels
// ---------------------------------------------------------------------------

/// Canonical channels for the `telemetry` envelope (G86 contract names).
///
/// Every channel is range-checked by [`add`]; an absent or unconfigured sensor
/// is omitted rather than reported as zero.
pub fn canonical(d: &SpeeduinoData) -> Channels {
    let mut c = Channels::new();
    add(&mut c, "rpm", f64::from(d.rpm));
    add(&mut c, "throttle", f64::from(d.tps));
    add(&mut c, "manifoldKpa", f64::from(d.map));
    add(&mut c, "batteryV", f64::from(d.battery_voltage()));
    add(&mut c, "coolantC", f64::from(d.coolant_celsius()));
    add(&mut c, "intakeC", f64::from(d.iat_celsius()));
    add(&mut c, "ignitionDeg", f64::from(d.advance));
    // An absent wideband reads zero, not a measured AFR of zero.
    if (50..=250).contains(&d.o2_primary) {
        add(&mut c, "afr", f64::from(d.o2_primary) / 10.0);
    }
    // Barometer is only meaningful when the ECU reports a plausible value.
    if (50..=120).contains(&d.baro) {
        add(&mut c, "baroKpa", f64::from(d.baro));
        add(&mut c, "boostKpa", f64::from(d.map) - f64::from(d.baro));
    }
    if d.afr_target > 0 {
        add(&mut c, "afrTarget", f64::from(d.afr_target_real()));
    }
    add(&mut c, "veCurrent", f64::from(d.ve_current));
    add(&mut c, "injectorPwMs", f64::from(d.pw1_ms()));
    add(&mut c, "dwellMs", f64::from(d.dwell_ms()));
    add(&mut c, "syncLossCount", f64::from(d.sync_loss_counter));
    c
}

// ---------------------------------------------------------------------------
// Status flags (speeduino/globals.h, firmware 202501.6)
// ---------------------------------------------------------------------------

/// One decoded status bit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Flag {
    pub group: &'static str,
    pub name: &'static str,
    pub on: bool,
}

const ENGINE: [&str; 8] = [
    "running",
    "cranking",
    "afterStartEnrich",
    "warmupEnrich",
    "tpsAccel",
    "tpsDecel",
    "mapAccel",
    "mapDecel",
];
const STATUS1: [&str; 8] = [
    "inj1",
    "inj2",
    "inj3",
    "inj4",
    "dfco",
    "boostCutFuel",
    "toothLog1Ready",
    "toothLog2Ready",
];
const STATUS2: [&str; 8] = [
    "hardLaunch",
    "softLaunch",
    "hardRevLimit",
    "softRevLimit",
    "boostCutSpark",
    "sparkError",
    "idleControl",
    "sync",
];
const STATUS3: [&str; 5] = [
    "resetPrevent",
    "nitrous",
    "fuel2Active",
    "vssRefresh",
    "halfSync",
];
const STATUS4: [&str; 8] = [
    "wmiEmpty",
    "vvt1Error",
    "vvt2Error",
    "fan",
    "burnPending",
    "stagingActive",
    "commsCompat",
    "allowLegacyComms",
];
const STATUS5: [&str; 6] = [
    "flatShiftHard",
    "flatShiftSoft",
    "spark2Active",
    "knockActive",
    "knockPulse",
    "tsOnSecondary",
];
const PROTECT: [&str; 5] = ["rpm", "map", "oil", "afr", "coolant"];
const AIRCON: [&str; 7] = [
    "request",
    "compressor",
    "rpmLockout",
    "tpsLockout",
    "turningOn",
    "cltLockout",
    "fan",
];

fn bits(out: &mut Vec<Flag>, group: &'static str, names: &[&'static str], value: u8) {
    for (bit, name) in names.iter().enumerate() {
        out.push(Flag {
            group,
            name,
            on: value & (1 << bit) != 0,
        });
    }
}

/// Decode every documented status bit of a packet.
pub fn flags(d: &SpeeduinoData) -> Vec<Flag> {
    let mut out = Vec::with_capacity(64);
    bits(&mut out, "engine", &ENGINE, d.engine);
    bits(&mut out, "status1", &STATUS1, d.status1);
    bits(&mut out, "status2", &STATUS2, d.status2);
    bits(&mut out, "status3", &STATUS3, d.status3);
    bits(&mut out, "status4", &STATUS4, d.status4);
    if let Some(v) = d.status5 {
        bits(&mut out, "status5", &STATUS5, v);
    }
    bits(&mut out, "engineProtect", &PROTECT, d.engine_protect_status);
    if let Some(v) = d.air_con_status {
        bits(&mut out, "airCon", &AIRCON, v);
    }
    out
}

/// Squirts per cycle from `status3` bits 5–7.
pub fn squirts_per_cycle(d: &SpeeduinoData) -> u8 {
    d.status3 >> 5
}

// ---------------------------------------------------------------------------
// Encoder (simulator and tests)
// ---------------------------------------------------------------------------

/// Values the simulator controls; everything else is zero.
#[derive(Debug, Clone, Copy, Default)]
pub struct SimValues {
    pub secl: u8,
    pub engine: u8,
    pub status2: u8,
    pub map_kpa: u16,
    pub iat_c: i16,
    pub coolant_c: i16,
    pub battery_v: f32,
    pub afr: f32,
    pub rpm: u16,
    pub ve: u8,
    pub afr_target: f32,
    pub advance: i8,
    pub tps: u8,
    pub baro: u8,
    pub pw1_ms: f32,
    pub dwell_ms: f32,
    pub loops_per_second: u16,
    pub vss_kmh: u16,
    pub gear: u8,
    pub oil_kpa: u8,
    pub fuel_kpa: u8,
}

/// Build a 130-byte 2025.01 `A` packet.
pub fn encode(v: &SimValues) -> Vec<u8> {
    let mut p = vec![0u8; PACKET_LEN];
    let put16 =
        |p: &mut Vec<u8>, at: usize, x: u16| p[at..at + 2].copy_from_slice(&x.to_le_bytes());
    let temp = |c: i16| (c + 40).clamp(0, 255) as u8;
    p[0] = v.secl;
    p[2] = v.engine;
    put16(&mut p, 4, v.map_kpa);
    p[6] = temp(v.iat_c);
    p[7] = temp(v.coolant_c);
    p[8] = 100;
    p[9] = (v.battery_v * 10.0).round().clamp(0.0, 255.0) as u8;
    p[10] = (v.afr * 10.0).round().clamp(0.0, 255.0) as u8;
    p[11] = 100;
    p[12] = 100;
    p[13] = 100;
    put16(&mut p, 14, v.rpm);
    put16(&mut p, 17, 100);
    p[19] = v.ve;
    p[21] = (v.afr_target * 10.0).round().clamp(0.0, 255.0) as u8;
    p[24] = v.advance as u8;
    p[25] = v.tps;
    put16(&mut p, 26, v.loops_per_second);
    put16(&mut p, 28, 4096);
    p[32] = v.status2;
    p[41] = v.baro;
    put16(&mut p, 76, (v.pw1_ms * 10.0).round() as u16);
    put16(&mut p, 90, (v.dwell_ms * 10.0).round() as u16);
    p[102] = v.ve;
    put16(&mut p, 104, v.vss_kmh);
    p[106] = v.gear;
    p[107] = v.fuel_kpa;
    p[108] = v.oil_kpa;
    p
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_channels_have_normalized_units_and_no_invented_brake_or_gear() {
        let mut packet = [0u8; 130];
        packet[6] = 65;
        packet[7] = 130;
        packet[9] = 138;
        packet[10] = 147;
        packet[14] = 0x70;
        packet[15] = 0x17;
        packet[25] = 72;
        packet[24] = (-12i8) as u8;
        packet[118] = (-8i8) as u8;
        packet[119] = (-6i8) as u8;
        packet[41] = 101;
        packet[4] = 180;
        let data = parse(&packet).unwrap();
        let c = canonical(&data);
        assert_eq!(c["rpm"], 6000.0);
        assert_eq!(c["ignitionDeg"], -12.0);
        assert_eq!(c["coolantC"], 90.0);
        assert_eq!(c["intakeC"], 25.0);
        assert!((c["batteryV"] - 13.8).abs() < 0.001);
        assert!((c["afr"] - 14.7).abs() < 0.001);
        assert_eq!(c["boostKpa"], 79.0);
        assert!(!c.contains_key("brake"));
        assert!(!c.contains_key("gear"));
        let legacy = get_params_to_publish(&data);
        assert!(legacy.contains(&("ADV", "-12".into())));
        assert!(legacy.contains(&("AD1", "-8".into())));
        assert!(legacy.contains(&("AD2", "-6".into())));
    }

    #[test]
    fn absent_wideband_and_barometer_are_omitted() {
        let c = canonical(&parse(&[0u8; 130]).unwrap());
        assert!(!c.contains_key("afr"));
        assert!(!c.contains_key("boostKpa"));
        assert!(!c.contains_key("afrTarget"));
    }

    #[test]
    fn real_ecu_packet_from_firmware_202501() {
        // Captured read-only from a DIY-EFI CORE4 (speeduino 202501) on USB power.
        let packet = include_bytes!("../../tests/fixtures/speeduino-202501-130.bin");
        assert_eq!(packet.len(), PACKET_LEN);
        let d = parse(packet).unwrap();
        assert_eq!(d.rpm, 0);
        assert_eq!(d.coolant_celsius(), 82);
        assert!(d.pw5.is_none(), "2025.01 sends no PW5–PW8");
        let flags = flags(&d);
        assert!(
            flags
                .iter()
                .any(|f| f.group == "engine" && f.name == "running")
        );
        assert!(
            !flags
                .iter()
                .any(|f| f.group == "engine" && f.name == "running" && f.on)
        );
    }

    #[test]
    fn status_bits_follow_globals_h() {
        let mut p = [0u8; 130];
        p[2] = 0b0000_0011; // running + cranking
        p[32] = 0b1000_0100; // hard rev limit + sync
        p[85] = 0b0001_0000; // coolant protection
        p[84] = 0b0100_0000; // nsquirts = 2
        let d = parse(&p).unwrap();
        let on: Vec<(&str, &str)> = flags(&d)
            .iter()
            .filter(|f| f.on)
            .map(|f| (f.group, f.name))
            .collect();
        assert_eq!(
            on,
            [
                ("engine", "running"),
                ("engine", "cranking"),
                ("status2", "hardRevLimit"),
                ("status2", "sync"),
                ("engineProtect", "coolant")
            ]
        );
        assert_eq!(squirts_per_cycle(&d), 2);
    }

    #[test]
    fn encoder_round_trips_through_the_parser() {
        let v = SimValues {
            rpm: 3500,
            map_kpa: 95,
            coolant_c: 88,
            iat_c: 31,
            battery_v: 13.9,
            afr: 13.2,
            tps: 42,
            baro: 100,
            advance: 28,
            ve: 77,
            pw1_ms: 4.2,
            dwell_ms: 3.1,
            afr_target: 13.0,
            ..SimValues::default()
        };
        let d = parse(&encode(&v)).unwrap();
        assert_eq!(
            (
                d.rpm,
                d.map,
                d.coolant_celsius(),
                d.iat_celsius(),
                d.tps,
                d.advance
            ),
            (3500, 95, 88, 31, 42, 28)
        );
        assert!((d.battery_voltage() - 13.9).abs() < 0.01);
        assert!((d.pw1_ms() - 4.2).abs() < 0.01);
        let c = canonical(&d);
        assert_eq!(c["boostKpa"], -5.0);
        assert!((c["afrTarget"] - 13.0).abs() < 0.01);
    }

    fn zero_packet() -> [u8; 130] {
        [0u8; 130]
    }

    // --- Length guards ---

    #[test]
    fn test_parse_too_short() {
        assert!(parse(&[0u8; 50]).is_err());
    }

    #[test]
    fn test_parse_empty() {
        assert!(parse(&[]).is_err());
    }

    #[test]
    fn test_parse_129_bytes_too_short() {
        assert!(parse(&[0u8; 129]).is_err());
    }

    #[test]
    fn test_parse_minimum_130_bytes() {
        assert!(parse(&[0u8; 130]).is_ok());
    }

    #[test]
    fn test_parse_130_bytes_has_emap() {
        // All 130-byte packets include EMAP (130 >= 123)
        let d = parse(&[0u8; 130]).unwrap();
        assert_eq!(d.emap, Some(0));
    }

    #[test]
    fn test_parse_130_bytes_has_extended_fields() {
        let d = parse(&[0u8; 130]).unwrap();
        assert!(d.knock_retard.is_some()); // byte 129 present
    }

    #[test]
    fn test_parse_137_bytes_has_pw5_no_pw8() {
        let d = parse(&[0u8; 137]).unwrap();
        assert!(d.pw5.is_some()); // 137 >= 132
        assert!(d.pw8.is_none()); // 137 < 138
    }

    #[test]
    fn test_parse_138_bytes_all_pw_fields() {
        let d = parse(&[0u8; 138]).unwrap();
        assert!(d.pw5.is_some());
        assert!(d.pw8.is_some());
    }

    // --- Exact byte mapping ---

    #[test]
    fn test_secl_byte_0() {
        let mut p = zero_packet();
        p[0] = 42;
        assert_eq!(parse(&p).unwrap().secl, 42);
    }

    #[test]
    fn test_sync_loss_counter_byte_3() {
        let mut p = zero_packet();
        p[3] = 7;
        assert_eq!(parse(&p).unwrap().sync_loss_counter, 7);
    }

    #[test]
    fn test_map_le_bytes_4_5() {
        let mut p = zero_packet();
        p[4] = 0x2C;
        p[5] = 0x01; // 300 kPa
        assert_eq!(parse(&p).unwrap().map, 300);
    }

    #[test]
    fn test_rpm_le_bytes_14_15() {
        let mut p = zero_packet();
        p[14] = 0xB8;
        p[15] = 0x0B; // 3000 RPM
        assert_eq!(parse(&p).unwrap().rpm, 3000);
    }

    #[test]
    fn test_corrections_two_bytes_17_18() {
        let mut p = zero_packet();
        p[17] = 0xE8;
        p[18] = 0x03; // 1000
        assert_eq!(parse(&p).unwrap().corrections, 1000);
    }

    #[test]
    fn test_ve1_byte_19_ve2_byte_20() {
        let mut p = zero_packet();
        p[19] = 85;
        p[20] = 90;
        let d = parse(&p).unwrap();
        assert_eq!(d.ve1, 85);
        assert_eq!(d.ve2, 90);
    }

    #[test]
    fn test_afr_target_byte_21() {
        let mut p = zero_packet();
        p[21] = 147;
        let d = parse(&p).unwrap();
        assert!((d.afr_target_real() - 14.7).abs() < 0.01);
    }

    #[test]
    fn test_tps_dot_u16_bytes_22_23() {
        let mut p = zero_packet();
        p[22] = 0xC8;
        p[23] = 0x00; // 200
        assert_eq!(parse(&p).unwrap().tps_dot, 200);
    }

    #[test]
    fn test_advance_byte_24() {
        let mut p = zero_packet();
        p[24] = 30;
        assert_eq!(parse(&p).unwrap().advance, 30);
    }

    #[test]
    fn test_tps_byte_25() {
        let mut p = zero_packet();
        p[25] = 75;
        assert_eq!(parse(&p).unwrap().tps, 75);
    }

    #[test]
    fn test_pw1_bytes_76_77() {
        let mut p = zero_packet();
        p[76] = 35;
        p[77] = 0; // 3.5 ms
        let d = parse(&p).unwrap();
        assert_eq!(d.pw1, 35);
        assert!((d.pw1_ms() - 3.5).abs() < 0.01);
    }

    #[test]
    fn test_dwell_bytes_90_91() {
        let mut p = zero_packet();
        p[90] = 45;
        p[91] = 0; // 4.5 ms
        let d = parse(&p).unwrap();
        assert_eq!(d.dwell, 45);
        assert!((d.dwell_ms() - 4.5).abs() < 0.01);
    }

    #[test]
    fn test_map_dot_u16_bytes_93_94() {
        let mut p = zero_packet();
        p[93] = 0x64;
        p[94] = 0x00; // 100
        assert_eq!(parse(&p).unwrap().map_dot, 100);
    }

    #[test]
    fn test_ve_current_byte_102() {
        let mut p = zero_packet();
        p[102] = 88;
        assert_eq!(parse(&p).unwrap().ve_current, 88);
    }

    #[test]
    fn test_ts_sd_status_byte_120() {
        let mut p = zero_packet();
        p[120] = 3;
        assert_eq!(parse(&p).unwrap().ts_sd_status, 3);
    }

    // --- CAN inputs ---

    #[test]
    fn test_canin_ch0_bytes_42_43() {
        let mut p = zero_packet();
        p[42] = 0x00;
        p[43] = 0x02; // 512
        assert_eq!(parse(&p).unwrap().canin[0], 512);
    }

    #[test]
    fn test_canin_ch15_bytes_72_73() {
        let mut p = zero_packet();
        p[72] = 0xFF;
        p[73] = 0x00; // 255
        assert_eq!(parse(&p).unwrap().canin[15], 255);
    }

    // --- Temperature offset ---

    #[test]
    fn test_iat_celsius() {
        let mut p = zero_packet();
        p[6] = 80; // 80 - 40 = 40°C
        assert_eq!(parse(&p).unwrap().iat_celsius(), 40);
    }

    #[test]
    fn test_coolant_celsius() {
        let mut p = zero_packet();
        p[7] = 125; // 85°C
        assert_eq!(parse(&p).unwrap().coolant_celsius(), 85);
    }

    #[test]
    fn test_zero_raw_temp_is_minus_40c() {
        let p = zero_packet();
        let d = parse(&p).unwrap();
        assert_eq!(d.iat_celsius(), -40);
        assert_eq!(d.coolant_celsius(), -40);
    }

    // --- Scaling helpers ---

    #[test]
    fn test_boost_target_times_2() {
        let mut p = zero_packet();
        p[30] = 100;
        assert_eq!(parse(&p).unwrap().boost_target_kpa(), 200);
    }

    #[test]
    fn test_tae_amount_times_2() {
        let mut p = zero_packet();
        p[16] = 25;
        assert_eq!(parse(&p).unwrap().tae_amount_pct(), 50);
    }

    #[test]
    fn test_battery_voltage_div_10() {
        let mut p = zero_packet();
        p[9] = 142; // 14.2 V
        assert!((parse(&p).unwrap().battery_voltage() - 14.2).abs() < 0.01);
    }

    // --- Implausible values parse; canonical() omits them ---

    #[test]
    fn test_validation_does_not_fail_on_overflow() {
        let mut p = zero_packet();
        p[14] = 0xFF;
        p[15] = 0xFF; // RPM = 65535
        assert!(parse(&p).is_ok());
    }

    // --- params list ---

    #[test]
    fn test_params_min_count() {
        let d = SpeeduinoData::default();
        let params = get_params_to_publish(&d);
        assert!(
            params.len() >= 80,
            "expected ≥80 params, got {}",
            params.len()
        );
    }

    #[test]
    fn test_params_rpm() {
        let d = SpeeduinoData {
            rpm: 3000,
            ..Default::default()
        };
        let params = get_params_to_publish(&d);
        let found = params.iter().find(|(k, _)| *k == "RPM").unwrap();
        assert_eq!(found.1, "3000");
    }

    #[test]
    fn test_params_battery_format() {
        let d = SpeeduinoData {
            battery_10: 142,
            ..Default::default()
        };
        let params = get_params_to_publish(&d);
        let found = params.iter().find(|(k, _)| *k == "BAT").unwrap();
        assert_eq!(found.1, "14.2");
    }

    #[test]
    fn test_params_emap_present_when_some() {
        let d = SpeeduinoData {
            emap: Some(101),
            ..Default::default()
        };
        let params = get_params_to_publish(&d);
        let found = params.iter().find(|(k, _)| *k == "EMP");
        assert!(found.is_some());
        assert_eq!(found.unwrap().1, "101");
    }

    #[test]
    fn test_params_emap_absent_when_none() {
        let d = SpeeduinoData::default();
        let params = get_params_to_publish(&d);
        assert!(!params.iter().any(|(k, _)| *k == "EMP"));
    }

    #[test]
    fn test_params_all_16_can_channels() {
        let mut d = SpeeduinoData::default();
        for i in 0..16 {
            d.canin[i] = i as u16 * 100;
        }
        let params = get_params_to_publish(&d);
        for i in 1..=16usize {
            let code = format!("CN{:02}", i);
            let found = params.iter().any(|(k, _)| *k == code.as_str());
            assert!(found, "missing param {}", code);
        }
    }
}
