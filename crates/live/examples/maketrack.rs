//! Write a test groove at a given tempo and sample rate: maketrack <out.wav> <bpm> <rate>
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let mut spec = obsidian_testaudio::dj_b();
    spec.bpm = a[2].parse().unwrap();
    spec.seed = 77;
    let t = obsidian_testaudio::track(&spec);
    let rate: u32 = a[3].parse().unwrap();
    let t = obsidian_audio_io::file::resample_offline(&t, 48_000, rate);
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(&a[1], spec).unwrap();
    for s in t {
        w.write_sample((s.clamp(-1.0, 1.0) * 32767.0) as i16)
            .unwrap();
    }
    w.finalize().unwrap();
}
