//! Sentence embeddings for the repetition scanner: the [`Embedder`] seam
//! plus its production implementation over candle.
//!
//! The trait is the stable seam (mirroring `LintPlugin`/`FakePlugin`):
//! scan-level repetition logic depends on `&dyn Embedder`, never on candle
//! types, so detection tests run hermetically with [`FakeEmbedder`].
//!
//! Model delivery is strictly user-supplied: [`CandleEmbedder::from_dir`]
//! loads `all-MiniLM-L6-v2` from a directory; nothing is ever downloaded.
//! Files whose sha256 differs from the pinned digests produce a stderr
//! warning naming both digests — the run continues (an updated revision of
//! the same model is not a hard error).

use std::path::Path;
use std::time::Instant;
use tracing::{debug, info, warn};

use candle_core::{Device, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::bert::{BertModel, Config};

use sha2::{Digest, Sha256};
use tokenizers::Tokenizer;
use wherror::Error;

/// Embedding failure modes. Colocated with the seam per the style guide.
#[derive(Debug, Error)]
#[error(debug)]
pub enum EmbedError {
    #[error("model directory {dir} is missing {file}")]
    MissingFile { dir: String, file: &'static str },
    #[error("failed to read {path}")]
    Io { path: String },
    #[error("tokenizer did not produce an encoding")]
    NoEncoding,
    #[error("candle error")]
    Candle(#[from] candle_core::Error),
}

/// One embedding per input, same order and count.
///
/// # Errors
///
/// Returns an error when the model cannot tokenize or infer; callers treat
/// a failure as "skip model-based repetition for this run".
pub trait Embedder {
    fn embed(&self, inputs: &[String]) -> Result<Vec<Vec<f32>>, error_stack::Report<EmbedError>>;
}

/// Vector-producing closure shared by [`FakeEmbedder`] instances.
type VectorFn = Box<dyn Fn(&str) -> Vec<f32> + Send + Sync>;

/// In-memory embedder for tests: each input maps through a user closure to
/// a deterministic vector. No model, no filesystem.
pub struct FakeEmbedder {
    vector_for: VectorFn,
}

impl FakeEmbedder {
    pub fn new(vector_for: impl Fn(&str) -> Vec<f32> + Send + Sync + 'static) -> Self {
        FakeEmbedder {
            vector_for: Box::new(vector_for),
        }
    }
}

impl Embedder for FakeEmbedder {
    fn embed(&self, inputs: &[String]) -> Result<Vec<Vec<f32>>, error_stack::Report<EmbedError>> {
        Ok(inputs.iter().map(|s| (self.vector_for)(s)).collect())
    }
}

/// Files the model directory must contain, with their pinned sha256 digests.
const MODEL_FILES: [(&str, &str); 5] = [
    (
        "model.safetensors",
        "53aa51172d142c89d9012cce15ae4d6cc0ca6895895114379cacb4fab128d9db",
    ),
    (
        "config.json",
        "953f9c0d463486b10a6871cc2fd59f223b2c70184f49815e7efbcab5d8908b41",
    ),
    (
        "tokenizer.json",
        "be50c3628f2bf5bb5e3a7f17b1f74611b2561a3a27eeab05e5aa30f411572037",
    ),
    (
        "tokenizer_config.json",
        "acb92769e8195aabd29b7b2137a9e6d6e25c476a4f15aa4355c233426c61576b",
    ),
    (
        "special_tokens_map.json",
        "303df45a03609e4ead04bc3dc1536d0ab19b5358db685b6f3da123d05ec200e3",
    ),
];

/// Tokenization limits: BERT's position table bounds input length, and
/// batches keep memory flat.
const MAX_LENGTH: usize = 512;
const BATCH_SIZE: usize = 16;

/// Production embedder: sentence-transformers `all-MiniLM-L6-v2` run
/// in-process on CPU via candle. Mean pooling over token embeddings
/// (attention-mask filtered), then L2 normalization.
pub struct CandleEmbedder {
    tokenizer: Tokenizer,
    model: BertModel,
}

impl std::fmt::Debug for CandleEmbedder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // BertModel does not implement Debug; identity is the tokenizer's.
        f.debug_struct("CandleEmbedder")
            .field("tokenizer", &self.tokenizer)
            .finish_non_exhaustive()
    }
}

impl CandleEmbedder {
    /// Load the model from a directory holding the five pinned files.
    ///
    /// # Errors
    ///
    /// Returns an error if a file is missing, unreadable, or the tokenizer /
    /// model fails to construct. Hash mismatches only warn (stderr).
    pub fn from_dir(dir: &Path) -> Result<Self, error_stack::Report<EmbedError>> {
        let started = Instant::now();
        info!(dir = %dir.display(), "loading embedding model all-MiniLM-L6-v2");
        for (file, pinned) in MODEL_FILES {
            let path = dir.join(file);
            if !path.is_file() {
                debug!(file = %path.display(), "model file missing");
                return Err(error_stack::Report::new(EmbedError::MissingFile {
                    dir: dir.display().to_string(),
                    file,
                }));
            }
            let digest = file_sha256(&path)?;
            if digest != pinned {
                warn!(
                    file = %path.display(),
                    actual = %digest,
                    expected = %pinned,
                    "model file sha256 differs from the pinned digest - continuing"
                );
                eprintln!(
                    "deslop: model file {} has sha256 {digest}; expected {pinned} - continuing",
                    path.display()
                );
            } else {
                debug!(file = %path.display(), sha256 = %digest, "model file verified");
            }
        }

        let tokenizer_path = dir.join("tokenizer.json");
        let tokenizer = Tokenizer::from_file(&tokenizer_path).map_err(|e| {
            error_stack::Report::new(EmbedError::Io {
                path: tokenizer_path.display().to_string(),
            })
            .attach(format!("tokenizer: {e}"))
        })?;
        // The MiniLM tokenizer.json carries the BERT post-processor; if a
        // user-supplied variant lacks it, add [CLS]/[SEP] manually or every
        // embedding degrades silently.
        let tokenizer = match tokenizer.get_post_processor() {
            Some(_) => tokenizer,
            None => {
                let mut t = tokenizer;
                t.with_post_processor(Some(tokenizers::processors::bert::BertProcessing::new(
                    ("[SEP]".to_string(), 102u32),
                    ("[CLS]".to_string(), 101u32),
                )));
                t
            }
        };

        debug!(
            elapsed_ms = started.elapsed().as_millis() as u64,
            "tokenizer ready"
        );

        let config_path = dir.join("config.json");
        let config_json = read_file(&config_path)?;
        let config: Config = serde_json::from_str(&config_json).map_err(|e| {
            error_stack::Report::new(EmbedError::Io {
                path: config_path.display().to_string(),
            })
            .attach(format!("config: {e}"))
        })?;

        let weights_path = dir.join("model.safetensors");
        let data = read_file_binary(&weights_path)?;
        debug!(
            bytes = data.len(),
            elapsed_ms = started.elapsed().as_millis() as u64,
            "weights read"
        );
        let device = Device::Cpu;
        let vb = VarBuilder::from_buffered_safetensors(data, candle_core::DType::F32, &device)
            .map_err(EmbedError::Candle)?;
        let model = BertModel::load(vb, &config).map_err(EmbedError::Candle)?;

        info!(
            elapsed_ms = started.elapsed().as_millis() as u64,
            hidden_size = config.hidden_size,
            layers = config.num_hidden_layers,
            "embedding model ready"
        );
        Ok(CandleEmbedder { tokenizer, model })
    }

    /// Tokenize one batch and run the forward pass; returns raw sentence
    /// vectors (pooled + normalized) for the batch.
    ///
    /// # Errors
    ///
    /// Returns an error if tokenization or inference fails.
    fn embed_batch(
        &self,
        batch: &[String],
    ) -> Result<Vec<Vec<f32>>, error_stack::Report<EmbedError>> {
        let encodings = self
            .tokenizer
            .encode_batch(batch.to_vec(), true)
            .map_err(|e| error_stack::Report::new(EmbedError::NoEncoding).attach(format!("{e}")))?;
        let max_len = encodings
            .iter()
            .map(|e| e.get_ids().len())
            .max()
            .unwrap_or(0)
            .min(MAX_LENGTH);
        let max_len = max_len.max(1);

        let mut ids_data = Vec::with_capacity(batch.len() * max_len);
        let mut mask_data = Vec::with_capacity(batch.len() * max_len);
        let mut type_data = Vec::with_capacity(batch.len() * max_len);
        for enc in &encodings {
            let ids: Vec<i64> = enc.get_ids().iter().map(|&i| i64::from(i)).collect();
            let types: Vec<i64> = enc.get_type_ids().iter().map(|&i| i64::from(i)).collect();
            let mask: Vec<i64> = enc
                .get_attention_mask()
                .iter()
                .map(|&i| i64::from(i))
                .collect();
            let len = ids.len().min(MAX_LENGTH);
            ids_data.extend_from_slice(&ids[..len]);
            type_data.extend_from_slice(&types[..len]);
            mask_data.extend_from_slice(&mask[..len]);
            let pad = max_len - len;
            let pad_id = i64::from(
                self.tokenizer
                    .get_vocab(true)
                    .get("[PAD]")
                    .copied()
                    .unwrap_or(0),
            );
            ids_data.extend(std::iter::repeat_n(pad_id, pad));
            type_data.extend(std::iter::repeat_n(0, pad));
            mask_data.extend(std::iter::repeat_n(0, pad));
        }

        let ids = Tensor::from_vec(ids_data, (batch.len(), max_len), &self.model.device)
            .map_err(EmbedError::Candle)?;
        let types = Tensor::from_vec(type_data, (batch.len(), max_len), &self.model.device)
            .map_err(EmbedError::Candle)?;
        let mask = Tensor::from_vec(mask_data, (batch.len(), max_len), &self.model.device)
            .map_err(EmbedError::Candle)?;

        let hidden = self
            .model
            .forward(&ids, &types, Some(&mask))
            .map_err(EmbedError::Candle)?;

        Ok(pool_and_normalize(&hidden, &mask).map_err(EmbedError::Candle)?)
    }
}

impl Embedder for CandleEmbedder {
    fn embed(&self, inputs: &[String]) -> Result<Vec<Vec<f32>>, error_stack::Report<EmbedError>> {
        if inputs.is_empty() {
            return Ok(Vec::new());
        }
        let started = Instant::now();
        debug!(
            inputs = inputs.len(),
            batch_size = BATCH_SIZE,
            "embedding batch run"
        );
        let mut out = Vec::with_capacity(inputs.len());
        for batch in inputs.chunks(BATCH_SIZE) {
            out.extend(self.embed_batch(batch)?);
        }
        debug!(
            inputs = inputs.len(),
            elapsed_ms = started.elapsed().as_millis() as u64,
            "embedding complete"
        );
        Ok(out)
    }
}

/// Mask-weighted mean pooling over token embeddings, then L2 normalization
/// per row (the sentence-transformers pooling convention). Returns one flat
/// vector per batch row.
fn pool_and_normalize(hidden: &Tensor, mask: &Tensor) -> candle_core::Result<Vec<Vec<f32>>> {
    let mask_f = mask.to_dtype(candle_core::DType::F32)?;
    // Mask-weighted sum over the sequence dim: [B, S, H] * [B, S, 1],
    // reduced with sum(D=1) -> [B, 1, H]; divide by the real token count.
    let weighted = hidden.broadcast_mul(&mask_f.unsqueeze(2)?)?;
    let summed = weighted.sum(1)?; // [B, H]
    let counts = mask_f.sum_keepdim(1)?.clamp(1f64, f64::INFINITY)?; // [B, 1, 1]
    let counts = counts.flatten_all()?; // [B]
    let pooled = summed.broadcast_div(&counts.unsqueeze(1)?)?; // [B, H]
    let norm = pooled
        .sqr()?
        .sum_keepdim(1)?
        .sqrt()?
        .clamp(1e-12f64, f64::INFINITY)?; // [B, 1]
    let normalized = pooled.broadcast_div(&norm)?; // [B, H]
    normalized.to_vec2::<f32>()
}

fn file_sha256(path: &Path) -> Result<String, error_stack::Report<EmbedError>> {
    let data = read_file_binary(path)?;
    let mut hasher = Sha256::new();
    hasher.update(&data);
    Ok(format!("{:x}", hasher.finalize()))
}

/// # Errors
///
/// Returns an error if the file cannot be read.
fn read_file(path: &Path) -> Result<String, error_stack::Report<EmbedError>> {
    std::fs::read_to_string(path).map_err(|_| {
        error_stack::Report::new(EmbedError::Io {
            path: path.display().to_string(),
        })
    })
}

/// # Errors
///
/// Returns an error if the file cannot be read.
fn read_file_binary(path: &Path) -> Result<Vec<u8>, error_stack::Report<EmbedError>> {
    std::fs::read(path).map_err(|_| {
        error_stack::Report::new(EmbedError::Io {
            path: path.display().to_string(),
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_embedder_returns_vectors_in_input_order() {
        // Given a fake keyed on input length.
        let fake = FakeEmbedder::new(|s| vec![s.len() as f32, 1.0]);

        // When embedding two inputs.
        let got = fake
            .embed(&["ab".to_owned(), "abcd".to_owned()])
            .expect("fake embeds");

        // Then vectors land in input order.
        assert_eq!(got, vec![vec![2.0, 1.0], vec![4.0, 1.0]]);
    }

    #[test]
    fn fake_embedder_empty_input_yields_no_vectors() {
        // Given no inputs.
        let fake = FakeEmbedder::new(|_| vec![1.0]);

        // When embedding.
        // Then the output is empty, not an error.
        assert_eq!(
            fake.embed(&[]).expect("fake embeds"),
            Vec::<Vec<f32>>::new()
        );
    }

    #[test]
    fn missing_model_dir_names_directory_and_file() {
        // Given an empty model directory.
        let dir = tempfile::tempdir().expect("tempdir");

        // When loading the embedder.
        let err = CandleEmbedder::from_dir(dir.path()).expect_err("missing files");

        // Then the error names the directory and the first missing file.
        let text = format!("{err:?}");
        assert!(text.contains("all-MiniLM-L6-v2") || text.contains(dir.path().to_str().unwrap()));
        assert!(text.contains("model.safetensors") || text.contains("config.json"));
    }

    #[test]
    #[ignore = "runs the real MiniLM model; opt in with --ignored and DESLOP_MODELS_DIR"]
    fn real_model_loads_and_embeds_deterministically() {
        // Given DESLOP_MODELS_DIR is unset or names no directory.
        let Some(dir) = model_dir_from_env() else {
            return; // not ignored here: the #[ignore] attribute gates this test
        };
        let embedder = CandleEmbedder::from_dir(&dir).expect("model loads");

        // When embedding two sentences twice.
        let inputs = vec![
            "Anthropic bought the books and scanned every page.".to_owned(),
            "They purchased the books and scanned each page.".to_owned(),
        ];
        let first = embedder.embed(&inputs).expect("embeds");
        let second = embedder.embed(&inputs).expect("embeds");

        // Then vectors are 384-dim, unit norm, and identical across runs.
        assert_eq!(first.len(), 2);
        for (a, b) in first.iter().zip(&second) {
            assert_eq!(a.len(), 384);
            let norm: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
            assert!((norm - 1.0).abs() < 1e-3, "unit norm, got {norm}");
            assert_eq!(a, b);
        }
        // And the two paraphrases land close together (cosine > 0.9).
        let cos: f32 = first[0].iter().zip(&first[1]).map(|(x, y)| x * y).sum();
        assert!(cos > 0.5, "paraphrases should be close, got {cos}");
    }

    fn model_dir_from_env() -> Option<std::path::PathBuf> {
        let dir = std::env::var("DESLOP_MODELS_DIR")
            .ok()
            .map(std::path::PathBuf::from)?;
        dir.is_dir().then_some(dir)
    }
}
