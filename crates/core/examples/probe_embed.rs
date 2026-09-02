use deslop_core::embedder::Embedder;

fn main() {
    let dir = std::path::PathBuf::from(std::env::var("HOME").unwrap())
        .join(".local/share/deslop/models/all-MiniLM-L6-v2");
    let t0 = std::time::Instant::now();
    let e = deslop_core::embedder::CandleEmbedder::from_dir(&dir, deslop_core::embedder::GpuBackend::Cpu).expect("load");
    println!("load: {:?}", t0.elapsed());
    let inputs: Vec<String> = (0..40)
        .map(|i| format!("Sentence number {i} about the Panama canal and books."))
        .collect();
    let t1 = std::time::Instant::now();
    let v = e.embed(&inputs).expect("embed");
    println!("embed 40: {:?} dim={}", t1.elapsed(), v[0].len());
}
