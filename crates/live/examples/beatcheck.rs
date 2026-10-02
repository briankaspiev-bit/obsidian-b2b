//! Check a recorded session: how far the partner (monitor.wav) sits from your own
//! beats (sent.wav), per 8 s window. Usage: beatcheck <session dir>
use obsidian_align::{beat_phase, estimate_period, onset_envelope, phase_lag};

fn load(p: &std::path::Path) -> Vec<f32> {
    hound::WavReader::open(p)
        .unwrap()
        .samples::<f32>()
        .map(|s| s.unwrap())
        .collect()
}

fn main() {
    let dir = std::path::PathBuf::from(std::env::args().nth(1).expect("session dir"));
    let (a, b) = (load(&dir.join("sent.wav")), load(&dir.join("monitor.wav")));
    let (ea, eb) = (onset_envelope(&a, 48), onset_envelope(&b, 48));
    let n = ea.len().min(eb.len());
    let hop_s = 0.001;
    let mut s = 0;
    let w: usize = std::env::args()
        .nth(2)
        .map(|v| v.parse().unwrap())
        .unwrap_or(8000);
    while s + w <= n {
        let (la, lb) = (&ea[s..s + w], &eb[s..s + w]);
        let loud = |v: &[f32]| v.iter().sum::<f32>() / v.len() as f32;
        if loud(la) > 1e-4 && loud(lb) > 1e-4 {
            if let Some(p) = estimate_period(la, hop_s, 70.0, 180.0) {
                if let Some((lag, conf)) = phase_lag(la, lb, p) {
                    let signed = if lag > p / 2.0 { lag - p } else { lag };
                    let fold = match (beat_phase(la, p), beat_phase(lb, p)) {
                        (Some(x), Some(y)) => (y - x + p / 2.0).rem_euclid(p) - p / 2.0,
                        _ => f64::NAN,
                    };
                    print!("fold {fold:+6.1} ms | ");
                    println!("{:>4}-{:<4}s  partner vs your beat {:+6.1} ms  (conf {:.1}, period {:.2} ms, partner period {:.2})", s / 1000, (s + w) / 1000, signed, conf, p, estimate_period(lb, hop_s, 70.0, 180.0).unwrap_or(0.0));
                }
            }
        }
        s += w / 2;
    }
}
