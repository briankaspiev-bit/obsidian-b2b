use crate::{check, dsp, merge, synth};

#[test]
fn xcorr_finds_known_lag() {
    let x: Vec<f32> = (0..4096).map(|i| synth::mix64(i) as f32 / u64::MAX as f32 - 0.5).collect();
    let mut y = vec![0.0f32; 5000];
    y[300..300 + 4096].copy_from_slice(&x);
    for phat in [false, true] {
        let (k, conf) = dsp::peak(&dsp::xcorr(&x, &y, phat));
        assert!((k - 300.0).abs() < 0.01, "phat={phat} k={k}");
        assert!(conf > 10.0);
    }
}

#[test]
fn sinc_reads_between_samples() {
    let f = 1000.0 / 48000.0;
    let x: Vec<f32> = (0..2000)
        .map(|i| (2.0 * std::f64::consts::PI * f * i as f64).sin() as f32)
        .collect();
    let s = dsp::Sinc::new();
    for pos in [500.25, 777.5, 1001.9] {
        let want = (2.0 * std::f64::consts::PI * f * pos).sin() as f32;
        assert!((s.at(&x, pos) - want).abs() < 1e-3, "pos={pos}");
    }
}

#[test]
fn robust_line_ignores_outliers() {
    let mut pts: Vec<(f64, f64)> = (0..20).map(|i| (i as f64 * 100.0, 5.0 + 1.0001 * i as f64 * 100.0)).collect();
    pts[7].1 += 400.0;
    let fit = dsp::robust_line(&pts, None, 1.0).unwrap();
    assert_eq!(fit.inliers, 19);
    assert!((fit.line.slope - 1.0001).abs() < 1e-9);
}

/// Short synthetic session end to end: generate, merge, score against truth.
#[test]
fn merge_recovers_synthetic_handoff() {
    let dir = std::env::temp_dir().join(format!("obm-test-{}", std::process::id()));
    synth::run(&synth::SynthOpts {
        out: dir.clone(),
        minutes: 3.0,
        handoffs: 1,
        seed: 3,
        bpm: 126.0,
        received: true,
        human_err_ms: 0.0,
        ppm: [60.0, -40.0],
        one_way_ms: [95.0, 130.0],
        loss: 0.02,
    })
    .unwrap();
    let opts = merge::Opts {
        method: merge::Method::Auto,
        gate_db: -60.0,
        target_lufs: Some(-14.0),
        ceiling_dbtp: -1.0,
        onset_search_ms: 200.0,
        stems: Some(dir.join("stems")),
        bits: 24,
    };
    let rep = merge::run(
        &dir.join("session.json"),
        &dir.join("master.wav"),
        Some(&dir.join("report.json")),
        &opts,
    )
    .unwrap();
    assert_eq!(rep.handoffs.len(), 1);
    assert_eq!(rep.handoffs[0].method, merge::Method::Received);
    assert!(rep.true_peak_dbtp_after <= -0.9);
    let s = check::run(
        &dir.join("truth.json"),
        &dir.join("report.json"),
        Some((&dir.join("session.json"), &dir.join("stems"))),
    )
    .unwrap();
    // The synthetic calibration values carry ~0.3 ms of noise, so allow 1 ms.
    assert!(s[0].map_err_max_abs_ms < 1.0, "map error {} ms", s[0].map_err_max_abs_ms);
    assert!(s[0].rendered_err_ms.unwrap().abs() < 1.0);
    let _ = std::fs::remove_dir_all(&dir);
}
