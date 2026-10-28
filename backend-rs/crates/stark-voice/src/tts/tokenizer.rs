//! Hugging Face `tokenizer.json` support, ported from the official pocket-tts Rust port.
//!
//! The app only ships the 18M model, whose tokenizer is a `tokenizer.json`. This deliberately
//! avoids the SentencePiece native dependency, which crashes in MinGW builds of the upstream
//! binary on Windows.

use anyhow::{Context, Result, bail};
use std::path::Path;
use std::sync::Arc;

pub struct Tokenizer {
    inner: tokenizers::Tokenizer,
}

impl Tokenizer {
    pub fn load(n_bins: usize, tokenizer_path: &Path) -> Result<Self> {
        log::info!("Loading tokenizers json from {}", tokenizer_path.display());
        let inner = tokenizers::Tokenizer::from_file(tokenizer_path)
            .map_err(|e| anyhow::anyhow!("{e}"))
            .with_context(|| format!("cannot load {}", tokenizer_path.display()))?;
        let vocab_size = inner.get_vocab_size(true);
        if vocab_size != n_bins {
            bail!("tokenizer has vocab size={vocab_size} but nbins={n_bins} was specified")
        }
        Ok(Self { inner })
    }

    pub fn encode(&self, text: &str) -> Result<Vec<u32>> {
        Ok(self
            .inner
            .encode(text, true)
            .map_err(|e| anyhow::anyhow!("{e}"))?
            .get_ids()
            .to_vec())
    }

    pub fn decode(&self, ids: &[u32]) -> Result<String> {
        self.inner
            .decode(ids, true)
            .map_err(|e| anyhow::anyhow!("{e}"))
    }
}

/// The tokenizer as xn-ptts's flow LM holds it, shared with the text chunking.
pub struct SharedTokenizer(pub Arc<Tokenizer>);

impl ptts::Tokenizer for SharedTokenizer {
    fn encode(&self, text: &str) -> xn::Result<Vec<u32>> {
        self.0
            .encode(text)
            .map_err(|e| xn::Error::msg(format!("{e:#}")))
    }

    fn decode(&self, tokens: &[u32]) -> xn::Result<String> {
        self.0
            .decode(tokens)
            .map_err(|e| xn::Error::msg(format!("{e:#}")))
    }
}
