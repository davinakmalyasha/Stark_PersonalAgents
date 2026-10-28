//! The YAML model configs of `pocket_tts/utils/config.py`, and their translation to the
//! `config.json` schema of xn-ptts.
//!
//! The configs of the Python package are embedded in the binary; any other YAML file, local or
//! remote, is read with the same schema, unknown fields being an error as with pydantic.

use anyhow::{Result, bail};
pub use ptts::flow_lm::FlowType;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;

/// The official English 18.2M config, pinned to the same revision as the weights.
pub const CONFIG_18M: &str = include_str!("../../configs/english_18m_2026-10.yaml");

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlowConfig {
    pub dim: usize,
    pub depth: usize,
    /// The sampler head, under the names pocket-tts gives them.
    #[serde(rename = "type", default)]
    pub kind: FlowType,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlowLMTransformerConfig {
    pub hidden_scale: usize,
    pub max_period: f64,
    pub d_model: usize,
    pub num_heads: usize,
    pub num_layers: usize,
    /// Width of the queries and of the attention output, `d_model` when `None`.
    #[serde(default)]
    pub attn_dim: Option<usize>,
    /// Key/value heads, each shared by `num_heads / num_kv_heads` query heads, `num_heads` when
    /// `None`.
    #[serde(default)]
    pub num_kv_heads: Option<usize>,
    /// Hidden width of the feed-forward layers, `d_model * hidden_scale` when `None`.
    #[serde(default)]
    pub dim_feedforward: Option<usize>,
}

impl FlowLMTransformerConfig {
    pub fn num_kv_heads(&self) -> usize {
        self.num_kv_heads.unwrap_or(self.num_heads)
    }

    pub fn head_dim(&self) -> usize {
        self.attn_dim.unwrap_or(self.d_model) / self.num_heads
    }

    pub fn dim_feedforward(&self) -> usize {
        self.dim_feedforward
            .unwrap_or(self.d_model * self.hidden_scale)
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LookupTable {
    pub dim: usize,
    pub n_bins: usize,
    pub tokenizer: String,
    pub tokenizer_path: String,
    /// A low-rank table: `rank`-wide embeddings and a bias-free projection to `dim`.
    #[serde(default)]
    pub rank: Option<usize>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlowLMConfig {
    /// Unused: the model always runs in float32 (or int8 with `--quantize`).
    #[allow(dead_code)]
    pub dtype: String,
    pub flow: FlowConfig,
    pub transformer: FlowLMTransformerConfig,
    pub lookup_table: LookupTable,
    #[serde(default)]
    pub weights_path: Option<String>,
    #[serde(default)]
    pub insert_bos_before_voice: bool,
    /// Width of the backbone output the sampler and EOS heads read: a bias-free projection from
    /// `d_model` when set.
    #[serde(default)]
    pub z_dim: Option<usize>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SEANetConfig {
    pub dimension: usize,
    pub channels: usize,
    pub n_filters: usize,
    pub n_residual_layers: usize,
    pub ratios: Vec<usize>,
    pub kernel_size: usize,
    pub residual_kernel_size: usize,
    pub last_kernel_size: usize,
    pub dilation_base: usize,
    pub pad_mode: String,
    pub compress: usize,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MimiTransformerConfig {
    pub d_model: usize,
    pub input_dimension: usize,
    pub output_dimensions: Vec<usize>,
    pub num_heads: usize,
    pub num_layers: usize,
    pub layer_scale: f64,
    pub context: usize,
    #[serde(default = "default_max_period")]
    pub max_period: f64,
    pub dim_feedforward: usize,
}

fn default_max_period() -> f64 {
    10000.0
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuantizerConfig {
    pub dimension: usize,
    pub output_dimension: usize,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MimiConfig {
    /// Unused: Mimi always runs in float32.
    #[allow(dead_code)]
    pub dtype: String,
    pub sample_rate: usize,
    pub channels: usize,
    pub frame_rate: f64,
    pub seanet: SEANetConfig,
    pub transformer: MimiTransformerConfig,
    pub quantizer: QuantizerConfig,
    #[serde(default)]
    pub weights_path: Option<String>,
    #[serde(default)]
    pub inner_dim: Option<usize>,
    #[serde(default)]
    pub outer_dim: Option<usize>,
    /// The decoder's SEANet and transformer, when they differ from the encoder's.
    #[serde(default)]
    pub decoder_seanet: Option<SEANetConfig>,
    #[serde(default)]
    pub decoder_transformer: Option<MimiTransformerConfig>,
}

impl MimiConfig {
    pub fn decoder_seanet(&self) -> &SEANetConfig {
        self.decoder_seanet.as_ref().unwrap_or(&self.seanet)
    }

    pub fn decoder_transformer(&self) -> &MimiTransformerConfig {
        self.decoder_transformer
            .as_ref()
            .unwrap_or(&self.transformer)
    }

    /// Whether the decoder has its own dims, which xn-ptts then loads with a config of its own.
    fn has_own_decoder(&self) -> bool {
        self.decoder_seanet.is_some() || self.decoder_transformer.is_some()
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub flow_lm: FlowLMConfig,
    pub mimi: MimiConfig,
    #[serde(default)]
    pub weights_path: Option<String>,
    #[serde(default)]
    pub weights_path_without_voice_cloning: Option<String>,
    #[serde(default)]
    pub pad_with_spaces_for_short_inputs: bool,
    #[serde(default)]
    pub remove_semicolons: bool,
    #[serde(default = "default_true")]
    pub append_terminal_punctuation: bool,
    #[serde(default = "default_true")]
    pub capitalize_first_letter: bool,
    /// Per-character rewrites applied before tokenization, `None` or "" deletes.
    #[serde(default)]
    pub replace_characters: BTreeMap<String, Option<String>>,
    #[serde(default)]
    pub model_recommended_frames_after_eos: Option<usize>,
    #[serde(default = "default_temperature")]
    pub default_temperature: f32,
    /// The EOS threshold when the caller does not pass one.
    #[serde(default = "default_eos_threshold")]
    pub default_eos_threshold: f32,
}

fn default_true() -> bool {
    true
}

fn default_temperature() -> f32 {
    0.3
}

fn default_eos_threshold() -> f32 {
    crate::tts::defaults::DEFAULT_EOS_THRESHOLD
}

impl Config {
    pub fn parse(yaml: &str) -> Result<Self> {
        let config: Self = serde_yaml::from_str(yaml)?;
        config.validate()?;
        Ok(config)
    }

    /// The official English 18.2M config. Asset paths are replaced by [`with_assets`] before
    /// the model is loaded, so this never touches the network.
    pub fn english_18m() -> Result<Self> {
        log::info!("Loading embedded english_18m_2026-10 config");
        Self::parse(CONFIG_18M)
    }

    /// Point the config at the local model and tokenizer files shipped with the app.
    pub fn with_assets(mut self, model: &Path, tokenizer: &Path) -> Self {
        self.weights_path = Some(model.to_string_lossy().into_owned());
        self.weights_path_without_voice_cloning = None;
        self.flow_lm.weights_path = None;
        self.mimi.weights_path = None;
        self.flow_lm.lookup_table.tokenizer = "tokenizers".to_string();
        self.flow_lm.lookup_table.tokenizer_path = tokenizer.to_string_lossy().into_owned();
        self
    }

    pub fn flow_type(&self) -> FlowType {
        self.flow_lm.flow.kind
    }

    /// The configuration is checked up front against what xn-ptts can run, so that a model it
    /// cannot represent fails with an explanation rather than a shape mismatch.
    fn validate(&self) -> Result<()> {
        let mimi = &self.mimi;
        let decoder_dimension = mimi.decoder_seanet().dimension;
        // TODO: upstream to xn-ptts. Its Mimi decoder upsamples from as many channels as it
        // decodes; an `outer_dim` (the `in_dimension` of `ConvTrUpsample1d`) other than that
        // needs support in `MimiConfig`.
        if mimi
            .outer_dim
            .is_some_and(|outer_dim| outer_dim != decoder_dimension)
        {
            bail!("mimi.outer_dim different from the decoder's seanet dimension is not supported")
        }
        for (name, seanet) in [
            ("seanet", &mimi.seanet),
            ("decoder_seanet", mimi.decoder_seanet()),
        ] {
            // TODO: upstream to xn-ptts. Its SEANet always pads with zeros.
            if seanet.pad_mode != "constant" {
                bail!(
                    "mimi.{name}.pad_mode {:?} is not supported",
                    seanet.pad_mode
                )
            }
            if seanet.channels != 1 {
                bail!("only mono Mimi models are supported")
            }
        }
        if mimi.decoder_seanet().ratios != mimi.seanet.ratios {
            bail!("mimi.decoder_seanet.ratios must be those of mimi.seanet")
        }
        for (name, transformer, dimension) in [
            ("transformer", &mimi.transformer, mimi.seanet.dimension),
            (
                "decoder_transformer",
                mimi.decoder_transformer(),
                decoder_dimension,
            ),
        ] {
            if transformer.input_dimension != dimension
                || transformer.output_dimensions != [dimension]
            {
                bail!("mimi.{name} must map its seanet dimension to itself")
            }
        }
        if mimi.channels != 1 {
            bail!("only mono Mimi models are supported")
        }
        for key in self.replace_characters.keys() {
            if key.chars().count() != 1 {
                bail!("string keys in translate table must be of length 1, got {key:?}")
            }
        }
        Ok(())
    }

    /// The `MimiConfig` of xn-ptts for a SEANet and a transformer of `self.mimi`.
    fn mimi_to_ptts(
        &self,
        seanet: &SEANetConfig,
        transformer: &MimiTransformerConfig,
    ) -> serde_json::Value {
        let mimi = &self.mimi;
        serde_json::json!({
            "channels": mimi.channels,
            "sample_rate": mimi.sample_rate,
            "frame_rate": mimi.frame_rate,
            "dimension": seanet.dimension,
            "quantizer_dimension": mimi.quantizer.dimension,
            "quantizer_output_dimension": mimi.quantizer.output_dimension,
            "n_filters": seanet.n_filters,
            "n_residual_layers": seanet.n_residual_layers,
            "ratios": seanet.ratios,
            "kernel_size": seanet.kernel_size,
            "last_kernel_size": seanet.last_kernel_size,
            "residual_kernel_size": seanet.residual_kernel_size,
            "dilation_base": seanet.dilation_base,
            "compress": seanet.compress,
            "transformer_d_model": transformer.d_model,
            "transformer_num_heads": transformer.num_heads,
            "transformer_num_layers": transformer.num_layers,
            "transformer_layer_scale": transformer.layer_scale,
            // pocket-tts (and moshi) count the current key in the `context` keys a query attends
            // to, where xn-ptts counts the `context` positions before it: one less is the same
            // window.
            "transformer_context": transformer.context.saturating_sub(1),
            "transformer_max_period": transformer.max_period,
            "transformer_dim_feedforward": transformer.dim_feedforward,
            "downsample_channel_wise": false,
            "inner_dim": mimi.inner_dim,
        })
    }

    /// The `config.json` of xn-ptts that describes the same model.
    pub fn to_ptts(
        &self,
        sampler_decode_steps: usize,
        eos_threshold: f32,
    ) -> Result<ptts::tts_model::TTSConfig> {
        let flow_lm = &self.flow_lm;
        let mimi = &self.mimi;
        let mut config = serde_json::json!({
            "flow_lm": {
                "d_model": flow_lm.transformer.d_model,
                "num_heads": flow_lm.transformer.num_heads,
                "num_layers": flow_lm.transformer.num_layers,
                "dim_feedforward": flow_lm.transformer.dim_feedforward(),
                "max_period": flow_lm.transformer.max_period,
                "n_bins": flow_lm.lookup_table.n_bins,
                "lut_dim": flow_lm.lookup_table.dim,
                "flow_dim": flow_lm.flow.dim,
                "flow_depth": flow_lm.flow.depth,
                "ldim": mimi.quantizer.dimension,
                "flow_type": flow_lm.flow.kind,
                "z_dim": flow_lm.z_dim,
                "attn_dim": flow_lm.transformer.attn_dim,
                "num_kv_heads": flow_lm.transformer.num_kv_heads,
                "lut_rank": flow_lm.lookup_table.rank,
            },
            "mimi": self.mimi_to_ptts(mimi.decoder_seanet(), mimi.decoder_transformer()),
            "lsd_decode_steps": sampler_decode_steps,
            "eos_threshold": eos_threshold,
            "model_id": null,
        });
        if mimi.has_own_decoder() {
            // xn-ptts encodes the voice prompt with `speaker_mimi` when it is set and decodes
            // with `mimi`: the encoder's config goes there, under the same weight prefix.
            config["speaker_mimi"] = serde_json::json!({
                "prefix": "mimi",
                "mimi": self.mimi_to_ptts(&mimi.seanet, &mimi.transformer),
            });
        }
        if flow_lm.insert_bos_before_voice {
            // pocket-tts prepends `bos_before_voice` to the voice embedding, which is what a
            // `speaker_wavs` prefix sink does in xn-ptts: `[empty text, sink, voice]`. The check
            // that comes with sinks wants the CFG null branch to have no audio, which pocket-tts
            // never runs anyway.
            config["prefix_sinks"] = serde_json::json!({ "speaker_wavs": {} });
            config["cfg_null_audio_empty"] = serde_json::json!(true);
        }
        Ok(serde_json::from_value(config)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_18m_config_parses_and_translates() {
        let config = Config::english_18m().unwrap();
        let ptts = config.to_ptts(1, -1.0).unwrap();
        assert_eq!(ptts.flow_lm.flow_type, FlowType::Drifting);
        assert_eq!(ptts.mimi.dimension, 128);
        assert_eq!(ptts.flow_lm.num_layers, 8);
    }

    #[test]
    fn assets_replace_every_remote_path() {
        let config = Config::english_18m()
            .unwrap()
            .with_assets(Path::new("model.safetensors"), Path::new("tokenizer.json"));
        assert_eq!(config.weights_path.as_deref(), Some("model.safetensors"));
        assert_eq!(config.flow_lm.lookup_table.tokenizer, "tokenizers");
        assert_eq!(config.flow_lm.lookup_table.tokenizer_path, "tokenizer.json");
        assert!(config.weights_path_without_voice_cloning.is_none());
    }

    #[test]
    fn unknown_fields_are_rejected() {
        let yaml = format!("{CONFIG_18M}\nnot_a_field: 1\n");
        assert!(Config::parse(&yaml).is_err());
    }
}
