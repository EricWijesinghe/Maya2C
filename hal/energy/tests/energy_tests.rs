//! Master Prompt 6 §8: energy protocol parsers against the specifications'
//! examples and hostile bytes, IEEE 1547 trip timing, Δf pricing and
//! certificate issuance.

#![allow(clippy::unwrap_used)]

use maya_energy::EnergyError;
use maya_energy::goose::{self, Value};
use maya_energy::ieee1547::{FREQUENCY_DEFAULTS, Monitor, VOLTAGE_DEFAULTS_CAT_II};
use maya_energy::market::{Certificate, CertificateRegistry, FrequencyPricing, WH_PER_CERTIFICATE};
use maya_energy::modbus::{self, Function, Mbap, Request, Response};

fn adu(transaction: u16, unit: u8, pdu: &[u8]) -> Vec<u8> {
    let len = u16::try_from(pdu.len() + 1).unwrap();
    [
        &transaction.to_be_bytes()[..],
        &[0, 0],
        &len.to_be_bytes(),
        &[unit],
        pdu,
    ]
    .concat()
}

#[test]
fn modbus_parses_the_specifications_own_examples() {
    // Modbus Application Protocol v1.1b3 §6.3: read holding registers 108-110.
    let request = adu(1, 0x11, &[0x03, 0x00, 0x6B, 0x00, 0x03]);
    let (header, pdu) = modbus::split_adu(&request).unwrap();
    assert_eq!(
        header,
        Mbap {
            transaction: 1,
            unit: 0x11
        }
    );
    let asked = modbus::parse_request(pdu).unwrap();
    assert_eq!(
        asked,
        Request::Read {
            function: Function::ReadHoldingRegisters,
            start: 0x6B,
            count: 3
        }
    );
    let response = [0x03, 0x06, 0x02, 0x2B, 0x00, 0x00, 0x00, 0x64];
    assert_eq!(
        modbus::parse_response(&response, &asked).unwrap(),
        Response::Registers {
            function: Function::ReadHoldingRegisters,
            values: vec![555, 0, 100]
        }
    );
    // §6.6 write single register, §6.12 write multiple registers.
    let single = modbus::parse_request(&[0x06, 0x00, 0x01, 0x00, 0x03]).unwrap();
    assert_eq!(
        single,
        Request::WriteSingle {
            address: 1,
            value: 3
        }
    );
    let multi =
        modbus::parse_request(&[0x10, 0x00, 0x01, 0x00, 0x02, 0x04, 0x00, 0x0A, 0x01, 0x02])
            .unwrap();
    assert_eq!(
        multi,
        Request::WriteMultiple {
            start: 1,
            values: vec![0x000A, 0x0102]
        }
    );
    assert_eq!(
        modbus::parse_response(&[0x10, 0x00, 0x01, 0x00, 0x02], &multi).unwrap(),
        Response::WroteMultiple { start: 1, count: 2 }
    );
    // §7: an exception (illegal data address).
    assert_eq!(
        modbus::parse_response(&[0x83, 0x02], &asked).unwrap(),
        Response::Exception {
            function: 0x03,
            code: 2
        }
    );
    // Encoding round-trips.
    let encoded = modbus::encode_request(header, &asked);
    assert_eq!(
        modbus::parse_request(modbus::split_adu(&encoded).unwrap().1).unwrap(),
        asked
    );
}

#[test]
fn modbus_refuses_what_the_specification_forbids() {
    let refused = |bytes: &[u8]| modbus::parse_request(bytes).is_err();
    assert!(refused(&[0x03, 0x00, 0x00, 0x00, 0x00]), "zero registers");
    assert!(refused(&[0x03, 0x00, 0x00, 0x00, 126]), "126 registers");
    assert!(
        refused(&[0x10, 0x00, 0x01, 0x00, 0x02, 0x03, 0, 0, 0]),
        "byte count disagrees"
    );
    assert!(refused(&[0x03, 0x00]), "truncated");
    assert!(refused(&[0x2B, 0x0E]), "unsupported function");
    let asked = Request::Read {
        function: Function::ReadInputRegisters,
        start: 0,
        count: 2,
    };
    assert!(
        modbus::parse_response(&[0x04, 0x02, 0, 1], &asked).is_err(),
        "one register for two"
    );
    assert!(
        modbus::parse_response(&[0x03, 0x04, 0, 1, 0, 2], &asked).is_err(),
        "wrong table"
    );
    let mut bad = adu(1, 1, &[0x03, 0, 0, 0, 1]);
    bad[5] += 1;
    assert!(modbus::split_adu(&bad).is_err(), "MBAP length disagrees");
    bad = adu(1, 1, &[0x03, 0, 0, 0, 1]);
    bad[3] = 1;
    assert!(modbus::split_adu(&bad).is_err(), "protocol id not 0");
    // Every prefix of a valid ADU is refused, never a panic.
    let good = adu(
        7,
        1,
        &[0x10, 0x00, 0x01, 0x00, 0x02, 0x04, 0x00, 0x0A, 0x01, 0x02],
    );
    for n in 0..good.len() {
        assert!(
            modbus::split_adu(&good[..n])
                .and_then(|(_, p)| modbus::parse_request(p))
                .is_err()
        );
    }
}

fn tlv(tag: u8, body: &[u8]) -> Vec<u8> {
    let len = body.len();
    let header = if len < 0x80 {
        vec![tag, u8::try_from(len).unwrap()]
    } else {
        vec![tag, 0x81, u8::try_from(len).unwrap()]
    };
    [header, body.to_vec()].concat()
}

/// A GOOSE frame built field by field from IEC 61850-8-1's `goosePdu`
/// definition (hand-encoded, not a capture from a relay).
fn goose_frame(entries: u8) -> Vec<u8> {
    goose_frame_with(&sample_data(), entries)
}

fn sample_data() -> Vec<u8> {
    [
        tlv(0x83, &[1]),                         // boolean: breaker closed
        tlv(0x87, &[8, 0x42, 0x48, 0x00, 0x00]), // float32: 50.0
        tlv(0x84, &[6, 0x40]),                   // bit-string: quality
        tlv(
            0xA2,
            &[tlv(0x85, &[0xFF, 0x38]), tlv(0x86, &[0x00, 0xC8])].concat(),
        ), // {-200, 200}
    ]
    .concat()
}

fn goose_frame_with(data: &[u8], entries: u8) -> Vec<u8> {
    let pdu = [
        tlv(0x80, b"IED1LD0/LLN0$GO$gcb01"),
        tlv(0x81, &[0x07, 0xD0]),
        tlv(0x82, b"IED1LD0/LLN0$ds01"),
        tlv(0x83, b"IED1_GOOSE1"),
        tlv(0x84, &[0x65, 0x00, 0x00, 0x00, 0x80, 0x00, 0x00, 0x0A]),
        tlv(0x85, &[0x05]),
        tlv(0x86, &[0x00]),
        tlv(0x87, &[0x00]),
        tlv(0x88, &[0x01]),
        tlv(0x89, &[0x00]),
        tlv(0x8A, &[entries]),
        tlv(0xAB, data),
    ]
    .concat();
    let goose_pdu = tlv(0x61, &pdu);
    let len = u16::try_from(8 + goose_pdu.len()).unwrap();
    [
        &0x0001u16.to_be_bytes()[..],
        &len.to_be_bytes(),
        &[0, 0, 0, 0],
        &goose_pdu,
    ]
    .concat()
}

#[test]
fn goose_decodes_a_data_set_and_refuses_hostile_frames() {
    let g = goose::decode(&goose_frame(4)).unwrap();
    assert_eq!(g.gocb_ref, "IED1LD0/LLN0$GO$gcb01");
    assert_eq!(
        (g.time_allowed_to_live, g.st_num, g.sq_num, g.conf_rev),
        (2000, 5, 0, 1)
    );
    assert_eq!(g.go_id.as_deref(), Some("IED1_GOOSE1"));
    assert_eq!(
        g.data,
        vec![
            Value::Bool(true),
            Value::Float(50.0),
            Value::Bits {
                unused: 6,
                bytes: vec![0x40]
            },
            Value::Structure(vec![Value::Int(-200), Value::Unsigned(200)]),
        ]
    );
    assert!(
        goose::decode(&goose_frame(3)).is_err(),
        "numDatSetEntries disagrees"
    );
    let frame = goose_frame(4);
    for n in 0..frame.len() {
        assert!(goose::decode(&frame[..n]).is_err(), "prefix of {n} bytes");
    }
    // A structure nested past the bound is refused; one within it decodes.
    let nest = |levels: usize| (0..levels).fold(tlv(0x83, &[1]), |inner, _| tlv(0xA2, &inner));
    assert!(goose::decode(&goose_frame_with(&nest(goose::MAX_DEPTH + 1), 1)).is_err());
    assert!(goose::decode(&goose_frame_with(&nest(goose::MAX_DEPTH), 1)).is_ok());
    // A 5-byte BER length.
    assert!(matches!(
        goose::decode(&[0, 1, 0, 13, 0, 0, 0, 0, 0x61, 0x85, 1, 2, 3]),
        Err(EnergyError::Malformed(_) | EnergyError::Truncated(_))
    ));
}

#[test]
fn ieee1547_trips_only_after_the_clearing_time() {
    let mut f = Monitor::new(FREQUENCY_DEFAULTS);
    assert!(f.sample(0, 60_000).unwrap().is_empty());
    // 62.5 Hz: OF2 after 160 ms, not before; OF1 still counting.
    assert!(f.sample(1_000, 62_500).unwrap().is_empty());
    assert!(f.sample(1_159, 62_500).unwrap().is_empty());
    let trips = f.sample(1_160, 62_500).unwrap();
    assert_eq!(trips.iter().map(|e| e.name).collect::<Vec<_>>(), ["OF2"]);
    // Back inside the band resets; a 59 Hz dip never trips UF1's 58.5 Hz.
    assert!(f.sample(1_200, 60_000).unwrap().is_empty());
    assert!(f.sample(400_000, 59_000).unwrap().is_empty());
    assert!(f.sample(1_000, 60_000).is_err(), "time went backwards");

    let mut v = Monitor::new(VOLTAGE_DEFAULTS_CAT_II);
    assert!(v.sample(0, 690).unwrap().is_empty());
    assert!(v.sample(9_999, 690).unwrap().is_empty());
    assert_eq!(v.sample(10_000, 690).unwrap()[0].name, "UV1");
}

#[test]
fn prices_follow_frequency_and_certificates_mint_once_per_mwh() {
    let pricing = FrequencyPricing {
        nominal_mhz: 60_000,
        base: 1_000,
        bps_per_mhz: 10,
        floor_bps: 5_000,
        cap_bps: 30_000,
    };
    assert_eq!(pricing.price(60_000), 1_000);
    assert_eq!(pricing.price(59_900), 1_100, "100 mHz short: +10%");
    assert_eq!(pricing.price(60_100), 900);
    assert_eq!(pricing.price(55_000), 3_000, "capped");
    assert_eq!(pricing.price(65_000), 500, "floored");

    let (meter, claim) = ([7; 32], [9; 32]);
    let mut registry = CertificateRegistry::default();
    registry.enroll(meter, 5_000_000).unwrap();
    assert!(registry.enroll(meter, 0).is_err());
    assert!(
        registry.record(meter, 5_999_999).unwrap().is_empty(),
        "under 1 MWh since enrollment"
    );
    let minted = registry
        .record(meter, 5_000_000 + 2 * WH_PER_CERTIFICATE + 1)
        .unwrap();
    assert_eq!(
        minted,
        vec![
            Certificate { meter, serial: 0 },
            Certificate { meter, serial: 1 }
        ]
    );
    assert!(
        registry
            .record(meter, 5_000_000 + 2 * WH_PER_CERTIFICATE + 1)
            .unwrap()
            .is_empty(),
        "no double mint"
    );
    assert!(registry.record(meter, 1).is_err(), "a meter run backwards");
    registry.retire(minted[0], claim).unwrap();
    assert!(
        registry.retire(minted[0], [1; 32]).is_err(),
        "retired twice"
    );
    assert!(
        registry
            .retire(Certificate { meter, serial: 2 }, claim)
            .is_err(),
        "never minted"
    );
    assert_eq!(registry.retired_by(&claim), 1);
}
