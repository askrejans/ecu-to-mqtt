//! Read-only ECU interoperability profiles. Sources and exact compatibility
//! boundaries are recorded in ECU_PROTOCOLS.md.
use crate::errors::{ParseError, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EcuProtocol {
    #[default]
    Speeduino,
    Ms2,
    Ms3,
    Ms3Pro,
    Microsquirt,
    MegasquirtCanDash,
    MegasquirtCanRealtime,
    HaltechCanV2,
    MaxxecuCanV12,
    MaxxecuCanV13,
    EcumasterEmuCan,
    AemnetCan,
    LinkGenericDash,
    LinkGenericDash2,
    MotecM1Pdm,
}
impl EcuProtocol {
    /// Every supported profile, in documentation order.
    pub const ALL: [EcuProtocol; 15] = [
        Self::Speeduino,
        Self::Ms2,
        Self::Ms3,
        Self::Ms3Pro,
        Self::Microsquirt,
        Self::MegasquirtCanDash,
        Self::MegasquirtCanRealtime,
        Self::HaltechCanV2,
        Self::MaxxecuCanV12,
        Self::MaxxecuCanV13,
        Self::EcumasterEmuCan,
        Self::AemnetCan,
        Self::LinkGenericDash,
        Self::LinkGenericDash2,
        Self::MotecM1Pdm,
    ];

    /// The `ecu_protocol` configuration value for this profile.
    pub fn name(self) -> &'static str {
        match self {
            Self::Speeduino => "speeduino",
            Self::Ms2 => "ms2",
            Self::Ms3 => "ms3",
            Self::Ms3Pro => "ms3_pro",
            Self::Microsquirt => "microsquirt",
            Self::MegasquirtCanDash => "megasquirt_can_dash",
            Self::MegasquirtCanRealtime => "megasquirt_can_realtime",
            Self::HaltechCanV2 => "haltech_can_v2",
            Self::MaxxecuCanV12 => "maxxecu_can_v12",
            Self::MaxxecuCanV13 => "maxxecu_can_v13",
            Self::EcumasterEmuCan => "ecumaster_emu_can",
            Self::AemnetCan => "aemnet_can",
            Self::LinkGenericDash => "link_generic_dash",
            Self::LinkGenericDash2 => "link_generic_dash2",
            Self::MotecM1Pdm => "motec_m1_pdm",
        }
    }

    /// Manufacturer-facing description used in the terminal dashboard header.
    pub fn label(self) -> &'static str {
        match self {
            Self::Speeduino => "Speeduino primary 'A'",
            Self::Ms2 => "MegaSquirt MS2/Extra",
            Self::Ms3 => "MegaSquirt MS3",
            Self::Ms3Pro => "MegaSquirt MS3Pro",
            Self::Microsquirt => "MicroSquirt",
            Self::MegasquirtCanDash => "MegaSquirt CAN dash",
            Self::MegasquirtCanRealtime => "MegaSquirt CAN realtime",
            Self::HaltechCanV2 => "Haltech CAN V2",
            Self::MaxxecuCanV12 => "MaxxECU CAN 1.2",
            Self::MaxxecuCanV13 => "MaxxECU CAN 1.3",
            Self::EcumasterEmuCan => "ECUMaster EMU CAN",
            Self::AemnetCan => "AEMnet v150609",
            Self::LinkGenericDash => "Link Generic Dash",
            Self::LinkGenericDash2 => "Link Generic Dash 2",
            Self::MotecM1Pdm => "MoTeC M1 PDM GPx 1.4",
        }
    }

    pub fn is_can(self) -> bool {
        !matches!(
            self,
            Self::Speeduino | Self::Ms2 | Self::Ms3 | Self::Ms3Pro | Self::Microsquirt
        )
    }
    pub fn extended(self) -> bool {
        self == Self::AemnetCan
    }
    pub fn default_can_base(self) -> u32 {
        match self {
            Self::MegasquirtCanDash => 1512,
            Self::MegasquirtCanRealtime => 1520,
            Self::HaltechCanV2 => 0x360,
            Self::MaxxecuCanV12 | Self::MaxxecuCanV13 => 0x520,
            Self::EcumasterEmuCan => 0x600,
            Self::AemnetCan => 0x01f0a000,
            Self::LinkGenericDash | Self::LinkGenericDash2 => 1000,
            Self::MotecM1Pdm => 0x118,
            _ => 0,
        }
    }
    pub fn last_can_offset(self) -> u32 {
        match self {
            Self::HaltechCanV2 => 0x80,
            Self::MaxxecuCanV12 | Self::MaxxecuCanV13 => 0x22,
            Self::AemnetCan => 7,
            Self::EcumasterEmuCan => 7,
            Self::LinkGenericDash => 0,
            Self::LinkGenericDash2 => 3,
            Self::MotecM1Pdm => 1,
            _ => 63,
        }
    }
    pub fn command(self) -> &'static [u8] {
        if self == Self::Speeduino {
            b"A"
        } else {
            b"a\0\x06"
        }
    }
    pub fn source(self) -> &'static str {
        match self {
            Self::Speeduino => "speeduino-primary-a",
            Self::Ms2 => "ms2-compat112",
            Self::Ms3 => "ms3-compat112",
            Self::Ms3Pro => "ms3pro-compat112",
            Self::Microsquirt => "microsquirt-compat112",
            Self::MegasquirtCanDash => "megasquirt-can-dash",
            Self::MegasquirtCanRealtime => "megasquirt-can-realtime",
            Self::HaltechCanV2 => "haltech-can-v2",
            Self::MaxxecuCanV12 => "maxxecu-can-v1.2",
            Self::MaxxecuCanV13 => "maxxecu-can-v1.3",
            Self::EcumasterEmuCan => "ecumaster-emu-can",
            Self::AemnetCan => "aemnet-v150609",
            Self::LinkGenericDash => "link-generic-dash",
            Self::LinkGenericDash2 => "link-generic-dash2",
            Self::MotecM1Pdm => "motec-m1-pdm-v1.4",
        }
    }
}
pub type Channels = BTreeMap<String, f64>;
pub(super) fn invalid(message: &str) -> crate::errors::AppError {
    ParseError::InvalidData {
        offset: 0,
        message: message.to_owned(),
    }
    .into()
}
pub(super) fn signed(data: &[u8], offset: usize) -> f64 {
    i16::from_be_bytes([data[offset], data[offset + 1]]) as f64
}
pub(super) fn unsigned(data: &[u8], offset: usize) -> f64 {
    u16::from_be_bytes([data[offset], data[offset + 1]]) as f64
}
fn celsius(deci_fahrenheit: f64) -> f64 {
    (deci_fahrenheit / 10.0 - 32.0) * 5.0 / 9.0
}
pub(super) fn add(channels: &mut Channels, name: &str, value: f64) {
    let (low, high) = match name {
        "rpm" => (0., 20000.),
        "throttle" => (0., 100.),
        "manifoldKpa" => (0., 1000.),
        "coolantC" | "intakeC" => (-50., 250.),
        "batteryV" => (0., 36.),
        "ignitionDeg" => (-100., 100.),
        "afr" => (5., 30.),
        "ecuSpeedKmh" | "wheelSpeedFlKmh" | "wheelSpeedFrKmh" | "wheelSpeedRlKmh"
        | "wheelSpeedRrKmh" => (0., 500.),
        "boostKpa" => (-100., 550.),
        "lambda" | "lambda2" => (0.4, 3.),
        "oilPressureKpa" | "fuelPressureKpa" => (0., 5000.),
        "brakePressureKpa" => (0., 20000.),
        "oilC" | "fuelC" | "transmissionC" | "differentialC" => (-50., 250.),
        "gear" => (-1., 12.),
        "brakeSwitch" | "clutchSwitch" => (0., 1.),
        "lateralG" | "longitudinalG" => (-10., 10.),
        _ => return,
    };
    if value.is_finite() && value >= low && value <= high {
        channels.insert(name.to_owned(), value);
    }
}

/// The documented a 00 06 subset is identical across MS2/Extra 3.3+, MS3 1.2+
/// and their MicroSquirt/MS3Pro derivatives. Full 'A' packets are intentionally
/// not fed to this decoder because their layout depends on the firmware INI.
pub fn decode_compat112(data: &[u8]) -> Result<Channels> {
    if data.len() != 112 {
        return Err(invalid(
            "MegaSquirt compatibility reply must be exactly 112 bytes",
        ));
    }
    let mut out = Channels::new();
    add(&mut out, "rpm", unsigned(data, 6));
    add(&mut out, "ignitionDeg", signed(data, 8) / 10.);
    add(&mut out, "manifoldKpa", signed(data, 18) / 10.);
    add(&mut out, "intakeC", celsius(signed(data, 20)));
    add(&mut out, "coolantC", celsius(signed(data, 22)));
    add(&mut out, "throttle", signed(data, 24) / 10.);
    add(&mut out, "batteryV", signed(data, 26) / 10.);
    add(&mut out, "afr", signed(data, 28) / 10.);
    Ok(out)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CanInputFrame {
    pub id: u32,
    #[serde(default)]
    pub extended: bool,
    pub data: Vec<u8>,
    #[serde(default, rename = "timestampMs")]
    pub timestamp_ms: Option<u64>,
}

/// Standard 11-bit frames; disabled groups simply produce no channels. Frames
/// are independent so an absent group can never refresh another group's data.
pub fn decode_can(protocol: EcuProtocol, base: u32, frame: &CanInputFrame) -> Result<Channels> {
    if !protocol.is_can()
        || frame.id > if frame.extended { 0x1fffffff } else { 0x7ff }
        || frame.data.len() != 8
    {
        return Err(invalid(
            "Expected a valid CAN data identifier and exactly eight bytes",
        ));
    }
    if frame.extended != protocol.extended() {
        return Ok(Channels::new());
    }
    if !matches!(
        protocol,
        EcuProtocol::MegasquirtCanDash | EcuProtocol::MegasquirtCanRealtime
    ) {
        return crate::can_profiles::decode(protocol, base, frame);
    }
    let mut out = Channels::new();
    let Some(group) = frame.id.checked_sub(base) else {
        return Ok(out);
    };
    let d = &frame.data;
    match (protocol, group) {
        (EcuProtocol::MegasquirtCanDash, 0) => {
            add(&mut out, "manifoldKpa", signed(d, 0) / 10.);
            add(&mut out, "rpm", unsigned(d, 2));
            add(&mut out, "coolantC", celsius(signed(d, 4)));
            add(&mut out, "throttle", signed(d, 6) / 10.);
        }
        (EcuProtocol::MegasquirtCanDash, 1) => {
            add(&mut out, "intakeC", celsius(signed(d, 4)));
            add(&mut out, "ignitionDeg", signed(d, 6) / 10.);
        }
        (EcuProtocol::MegasquirtCanDash, 2) => add(&mut out, "afr", d[1] as f64 / 10.),
        (EcuProtocol::MegasquirtCanDash, 3) => add(&mut out, "batteryV", signed(d, 0) / 10.),
        (EcuProtocol::MegasquirtCanRealtime, 0) => add(&mut out, "rpm", unsigned(d, 6)),
        (EcuProtocol::MegasquirtCanRealtime, 1) => add(&mut out, "ignitionDeg", signed(d, 0) / 10.),
        (EcuProtocol::MegasquirtCanRealtime, 2) => {
            add(&mut out, "manifoldKpa", signed(d, 2) / 10.);
            add(&mut out, "intakeC", celsius(signed(d, 4)));
            add(&mut out, "coolantC", celsius(signed(d, 6)));
        }
        (EcuProtocol::MegasquirtCanRealtime, 3) => {
            add(&mut out, "throttle", signed(d, 0) / 10.);
            add(&mut out, "batteryV", signed(d, 2) / 10.);
            add(&mut out, "afr", signed(d, 4) / 10.);
        }
        _ => {}
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn profile_names_match_the_accepted_configuration_values() {
        for protocol in EcuProtocol::ALL {
            let value = serde_json::to_string(&protocol).unwrap();
            assert_eq!(value, format!("\"{}\"", protocol.name()));
            assert_eq!(
                serde_json::from_str::<EcuProtocol>(&value).unwrap(),
                protocol
            );
            assert!(!protocol.label().is_empty());
        }
    }
    #[test]
    fn all_serial_profiles_use_only_the_documented_read_command() {
        for protocol in [
            EcuProtocol::Ms2,
            EcuProtocol::Ms3,
            EcuProtocol::Ms3Pro,
            EcuProtocol::Microsquirt,
        ] {
            assert_eq!(protocol.command(), &[0x61, 0, 6]);
        }
    }
    #[test]
    fn serial_fixture_converts_signed_fahrenheit_and_units() {
        let mut bytes = [0u8; 112];
        for (offset, value) in [
            (6, 6000i16),
            (8, -125),
            (18, 1013),
            (20, 770),
            (22, 1940),
            (24, 723),
            (26, 138),
            (28, 147),
        ] {
            bytes[offset..offset + 2].copy_from_slice(&value.to_be_bytes());
        }
        let values = decode_compat112(&bytes).unwrap();
        assert_eq!(values["rpm"], 6000.);
        assert_eq!(values["intakeC"], 25.);
        assert_eq!(values["coolantC"], 90.);
        assert_eq!(values["ignitionDeg"], -12.5);
        assert_eq!(values["batteryV"], 13.8);
        assert!(!values.contains_key("brake"));
        assert!(decode_compat112(&bytes[..111]).is_err());
        assert!(decode_compat112(&[0; 113]).is_err());
    }
    #[test]
    fn dash_can_respects_base_and_group_boundaries() {
        let frame = CanInputFrame {
            id: 1512,
            extended: false,
            data: vec![0x03, 0xf5, 0x17, 0x70, 0x07, 0x94, 0x02, 0xd3],
            timestamp_ms: None,
        };
        let values = decode_can(EcuProtocol::MegasquirtCanDash, 1512, &frame).unwrap();
        assert_eq!(values["rpm"], 6000.);
        assert_eq!(values["coolantC"], 90.);
        assert_eq!(values["throttle"], 72.3);
        assert!(!values.contains_key("batteryV"));
        assert!(
            decode_can(EcuProtocol::MegasquirtCanDash, 1520, &frame)
                .unwrap()
                .is_empty()
        );
        assert!(
            decode_can(
                EcuProtocol::MegasquirtCanDash,
                1512,
                &CanInputFrame {
                    id: 1512,
                    extended: false,
                    data: vec![0; 7],
                    timestamp_ms: None,
                }
            )
            .is_err()
        );
    }
    #[test]
    fn realtime_packet_uses_different_offsets_than_dash() {
        let frame = CanInputFrame {
            id: 1520,
            extended: false,
            data: vec![0, 1, 0, 0, 0, 0, 0x17, 0x70],
            timestamp_ms: None,
        };
        let values = decode_can(EcuProtocol::MegasquirtCanRealtime, 1520, &frame).unwrap();
        assert_eq!(values["rpm"], 6000.);
        assert_eq!(values.len(), 1);
    }
}
