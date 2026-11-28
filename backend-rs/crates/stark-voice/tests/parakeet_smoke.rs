#[cfg(feature = "stt")]
#[test]
#[ignore = "needs the bundled Parakeet model"]
fn parakeet_transcribes_a_wav() {
    let dir = std::env::var("STARK_MODELS_DIR")
        .expect("set STARK_MODELS_DIR to the shared models directory");
    let audio =
        std::env::var("STARK_STT_SMOKE_WAV").expect("set STARK_STT_SMOKE_WAV to a 16 kHz WAV");
    let stt = stark_voice::SttService::load(std::path::Path::new(&dir)).unwrap();
    let text = stt.transcribe_file(std::path::Path::new(&audio)).unwrap();
    println!("{}", text);
    assert!(!text.trim().is_empty());
}
