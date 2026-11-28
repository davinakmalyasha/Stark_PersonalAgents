#[cfg(feature = "stt")]
#[test]
#[ignore = "needs the bundled Parakeet model"]
fn parakeet_transcribes_a_wav() {
    let dir = std::env::var("STARK_MODELS_DIR")
        .expect("set STARK_MODELS_DIR to the shared models directory");
    let audio =
        std::env::var("STARK_STT_SMOKE_WAV").expect("set STARK_STT_SMOKE_WAV to a 16 kHz WAV");
    let stt = stark_voice::SttService::load(std::path::Path::new(&dir)).unwrap();
    let text = stt
        .transcribe_file(
            std::path::Path::new(&audio),
            &stark_voice::Vocabulary::default(),
        )
        .unwrap()
        .expect("the smoke clip holds speech");
    println!("{}", text);
    assert!(!text.trim().is_empty());
}

#[cfg(feature = "stt")]
#[test]
#[ignore = "needs the bundled Parakeet model"]
fn parakeet_accepts_vocabulary_bias() {
    let dir = std::env::var("STARK_MODELS_DIR")
        .expect("set STARK_MODELS_DIR to the shared models directory");
    let audio =
        std::env::var("STARK_STT_SMOKE_WAV").expect("set STARK_STT_SMOKE_WAV to a 16 kHz WAV");
    let stt = stark_voice::SttService::load(std::path::Path::new(&dir)).unwrap();
    assert!(stt.vad_installed(), "Silero VAD is installed for the gate");
    let vocabulary = stark_voice::Vocabulary {
        entries: vec![
            stark_voice::VocabularyEntry {
                term: "Dungaringa".into(),
                sounds_like: vec!["dunga ringa".into()],
            },
            stark_voice::VocabularyEntry {
                term: "Stark".into(),
                sounds_like: Vec::new(),
            },
            // The clip says "Test"; the engine's exact-spelling pass must
            // swap a listed mishearing even when the word is ordinary.
            stark_voice::VocabularyEntry {
                term: "Kit".into(),
                sounds_like: vec!["Test".into()],
            },
        ],
        boost: stark_voice::VocabularyBoost::Strong,
    };
    let text = stt
        .transcribe_file(std::path::Path::new(&audio), &vocabulary)
        .unwrap()
        .expect("the smoke clip holds speech");
    println!("{text}");
    assert!(!text.trim().is_empty());
    assert!(
        text.contains("Kit"),
        "the exact sounds-like spelling became its term: {text:?}"
    );
    assert!(
        !text.contains("Test"),
        "the listed mishearing was replaced: {text:?}"
    );
}
