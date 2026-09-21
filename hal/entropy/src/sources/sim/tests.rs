use super::*;
use crate::health::{HealthFailure, HealthMonitor};

fn sources() -> Vec<Box<dyn EntropySource>> {
    vec![
        Box::new(ThermalNoise::typical()),
        Box::new(MicroVoltage::typical()),
        Box::new(Brownian::typical()),
        Box::new(HomodyneQrng::typical()),
    ]
}

#[test]
fn every_sim_source_declares_itself() {
    for source in sources() {
        assert_eq!(source.class(), SourceClass::Sim);
        assert!(source.name().starts_with("sim-"), "{}", source.name());
    }
}

#[test]
fn every_sim_source_passes_its_own_health_tests() {
    for mut source in sources() {
        let mut buf = vec![0u8; 200_000];
        source.fill(&mut buf).expect("fill");
        let mut monitor = HealthMonitor::new(source.min_entropy_millibits());
        assert_eq!(monitor.check(&buf), Ok(()), "{}", source.name());
    }
}

#[test]
fn declared_min_entropy_follows_the_physics() {
    // Thermal at typical settings spans hundreds of ADC steps: near 8 bits.
    let thermal = ThermalNoise::typical().min_entropy_millibits();
    // Micro-voltage jitter is ~2.5 steps: a few bits.
    let voltage = MicroVoltage::typical().min_entropy_millibits();
    // More clearance means less electronic noise to discount: more entropy.
    let qrng_low = HomodyneQrng::new(5.0, 4.0).min_entropy_millibits();
    let qrng_high = HomodyneQrng::new(20.0, 4.0).min_entropy_millibits();
    assert!(thermal > 6_500, "thermal {thermal}");
    assert!((1_500..3_500).contains(&voltage), "voltage {voltage}");
    assert!(qrng_high >= qrng_low, "{qrng_low} vs {qrng_high}");
    assert!((4_000..7_000).contains(&HomodyneQrng::typical().min_entropy_millibits()));
}

#[test]
fn the_empirical_distribution_respects_the_declared_min_entropy() {
    // The most frequent byte in 1,000,000 samples must not be much more
    // frequent than the declaration allows (a 25 % statistical allowance).
    for mut source in sources() {
        let mut buf = vec![0u8; 1_000_000];
        source.fill(&mut buf).expect("fill");
        let mut counts = [0u32; 256];
        for &b in &buf {
            counts[usize::from(b)] += 1;
        }
        let observed = f64::from(*counts.iter().max().expect("256 bins")) / buf.len() as f64;
        let allowed = (-f64::from(source.min_entropy_millibits()) / 1_000.0).exp2();
        assert!(
            observed <= allowed * 1.25,
            "{}: p_max {observed:.5} > {allowed:.5}",
            source.name()
        );
    }
}

#[test]
fn injected_faults_are_caught() {
    let mut stuck = Faulty::new(ThermalNoise::typical(), Fault::StuckAt(0x42), 1_000);
    let mut buf = vec![0u8; 4_096];
    stuck.fill(&mut buf).expect("fill");
    let mut monitor = HealthMonitor::new(stuck.min_entropy_millibits());
    assert!(matches!(
        monitor.check(&buf),
        Err(HealthFailure::RepetitionCount { value: 0x42, .. })
    ));

    let mut biased = Faulty::new(
        HomodyneQrng::typical(),
        Fault::Bias {
            value: 0x80,
            every: 3,
        },
        0,
    );
    let mut buf = vec![0u8; 4_096];
    biased.fill(&mut buf).expect("fill");
    let mut monitor = HealthMonitor::new(biased.min_entropy_millibits());
    assert!(matches!(
        monitor.check(&buf),
        Err(HealthFailure::AdaptiveProportion { .. })
    ));
}

#[test]
fn the_casimir_source_is_research_and_unavailable() {
    let mut casimir = crate::sources::research::CasimirCavity;
    assert_eq!(casimir.class(), SourceClass::Research);
    assert!(casimir.fill(&mut [0u8; 8]).is_err());
}
