//! CKIP Transformers word segmentation (BERT token classification, B/I labels),
//! run on CPU through candle.
//!
//! Model: ckiplab/bert-tiny-chinese-ws (Academia Sinica CKIP Lab, GPL-3.0),
//! trained on the Traditional-Chinese ASBC corpus. Simplified input is folded
//! character-by-character to Traditional before the vocabulary lookup, which is
//! length-preserving so boundaries map straight back onto the original text.

use crate::script;
use candle_core::{DType, Device, Tensor};
use candle_nn::{Linear, Module, VarBuilder};
use candle_transformers::models::bert::{BertModel, Config};
use std::collections::HashMap;
use std::path::Path;

/// Longest piece fed to the model at once (in chars); long chunks are split
/// at punctuation where possible. Well under BERT's 512-token limit.
const MAX_PIECE: usize = 126;

pub struct Ckip {
    bert: BertModel,
    cls: Linear,
    vocab: HashMap<char, u32>,
    unk: u32,
    cls_id: u32,
    sep_id: u32,
}

fn err<E: std::fmt::Display>(e: E) -> String {
    format!("CKIP model: {e}")
}

impl Ckip {
    fn from_vb(vb: VarBuilder, config_json: &str, vocab_txt: &str) -> Result<Self, String> {
        let cfg: Config = serde_json::from_str(config_json).map_err(err)?;
        let bert = BertModel::load(vb.clone(), &cfg).map_err(err)?;
        let cls = candle_nn::linear(cfg.hidden_size, 2, vb.pp("classifier")).map_err(err)?;
        let mut vocab = HashMap::new();
        let mut special = HashMap::new();
        for (i, tok) in vocab_txt.lines().enumerate() {
            let mut cs = tok.chars();
            match (cs.next(), cs.next()) {
                (Some(c), None) => {
                    vocab.entry(c).or_insert(i as u32);
                }
                _ if tok.starts_with('[') => {
                    special.insert(tok.to_owned(), i as u32);
                }
                _ => {}
            }
        }
        let get = |k: &str| {
            special
                .get(k)
                .copied()
                .ok_or_else(|| format!("CKIP vocab lacks {k}"))
        };
        Ok(Ckip {
            bert,
            cls,
            vocab,
            unk: get("[UNK]")?,
            cls_id: get("[CLS]")?,
            sep_id: get("[SEP]")?,
        })
    }

    /// Load the original Hugging Face checkpoint directory (pytorch_model.bin, config.json, vocab.txt).
    pub fn from_pth_dir(dir: &Path) -> Result<Self, String> {
        let vb = VarBuilder::from_pth(dir.join("pytorch_model.bin"), DType::F32, &Device::Cpu)
            .map_err(err)?;
        let cfg = std::fs::read_to_string(dir.join("config.json")).map_err(err)?;
        let vocab = std::fs::read_to_string(dir.join("vocab.txt")).map_err(err)?;
        Self::from_vb(vb, &cfg, &vocab)
    }

    fn id(&self, c: char) -> u32 {
        self.vocab.get(&c).copied().unwrap_or(self.unk)
    }

    /// Segment a batch of pieces (each at most [`MAX_PIECE`] chars) in one forward pass.
    /// The model sees `model_text[i]` (same char count as `batch[i]`); boundaries are cut in `batch[i]`.
    fn forward_batch<'a>(
        &self,
        batch: &[&'a str],
        model_text: &[String],
    ) -> Result<Vec<Vec<&'a str>>, String> {
        let lens: Vec<usize> = batch.iter().map(|s| s.chars().count()).collect();
        let width = lens.iter().copied().max().unwrap_or(0) + 2;
        let mut ids = Vec::with_capacity(batch.len() * width);
        let mut mask = Vec::with_capacity(batch.len() * width);
        for (m, &n) in model_text.iter().zip(&lens) {
            ids.push(self.cls_id);
            ids.extend(m.chars().map(|c| self.id(c)));
            ids.push(self.sep_id);
            ids.extend(std::iter::repeat_n(0, width - n - 2));
            mask.extend(std::iter::repeat_n(1u32, n + 2));
            mask.extend(std::iter::repeat_n(0u32, width - n - 2));
        }
        let shape = (batch.len(), width);
        let ids = Tensor::from_vec(ids, shape, &Device::Cpu).map_err(err)?;
        let mask = Tensor::from_vec(mask, shape, &Device::Cpu).map_err(err)?;
        let tt = ids.zeros_like().map_err(err)?;
        let h = self.bert.forward(&ids, &tt, Some(&mask)).map_err(err)?;
        let logits: Vec<Vec<Vec<f32>>> = self
            .cls
            .forward(&h)
            .and_then(|t| t.to_vec3())
            .map_err(err)?;
        Ok(batch
            .iter()
            .enumerate()
            .map(|(bi, s)| {
                // CKIP's decoding rule: a char starts a new word iff logit(B) > logit(I).
                let mut out = Vec::new();
                let mut start = 0usize;
                for (ci, (off, _)) in s.char_indices().enumerate() {
                    let row = &logits[bi][ci + 1];
                    if ci > 0 && row[0] > row[1] {
                        out.push(&s[start..off]);
                        start = off;
                    }
                }
                if start < s.len() {
                    out.push(&s[start..]);
                }
                out
            })
            .collect())
    }

    /// Segment whitespace-free chunks of any length. Runs on the calling thread
    /// (callers parallelise across groups of paragraphs).
    pub fn segment_many<'a>(&self, chunks: &[&'a str], fold_s2t: bool) -> Vec<Vec<&'a str>> {
        // Split into pieces, remembering which chunk each piece belongs to.
        let mut pieces: Vec<(usize, &'a str)> = Vec::new();
        for (ci, c) in chunks.iter().enumerate() {
            for p in split_pieces(c) {
                pieces.push((ci, p));
            }
        }
        let mut out: Vec<Vec<&'a str>> = vec![Vec::new(); chunks.len()];
        // Batch pieces of similar length to limit padding.
        let mut order: Vec<usize> = (0..pieces.len()).collect();
        order.sort_by_key(|&i| pieces[i].1.len());
        let mut seg: Vec<Vec<&'a str>> = vec![Vec::new(); pieces.len()];
        for idx in order.chunks(8) {
            let batch: Vec<&'a str> = idx.iter().map(|&i| pieces[i].1).collect();
            let model_text: Vec<String> = batch
                .iter()
                .map(|s| {
                    if fold_s2t {
                        script::fold_s2t(s)
                    } else {
                        (*s).to_owned()
                    }
                })
                .collect();
            match self.forward_batch(&batch, &model_text) {
                Ok(r) => {
                    for (&i, words) in idx.iter().zip(r) {
                        seg[i] = words;
                    }
                }
                Err(_) => {
                    for &i in idx {
                        seg[i] = pieces[i].1.split_inclusive(|_| true).collect();
                    }
                }
            }
        }
        for (i, (ci, _)) in pieces.iter().enumerate() {
            out[*ci].append(&mut seg[i]);
        }
        out
    }
}

/// Split at sentence/clause punctuation into pieces of at most MAX_PIECE chars.
fn split_pieces(s: &str) -> Vec<&str> {
    let mut v = Vec::new();
    let (mut start, mut n, mut last_punct) = (0usize, 0usize, None::<usize>);
    for (i, c) in s.char_indices() {
        n += 1;
        if matches!(
            c,
            '。' | '！' | '？' | '；' | '，' | '：' | '、' | '…' | '!' | '?' | ';' | ','
        ) {
            last_punct = Some(i + c.len_utf8());
        }
        if n >= MAX_PIECE {
            let cut = last_punct
                .filter(|&p| p > start)
                .unwrap_or(i + c.len_utf8());
            v.push(&s[start..cut]);
            n = s[cut..i + c.len_utf8()].chars().count();
            start = cut;
            last_punct = None;
        }
    }
    if start < s.len() {
        v.push(&s[start..]);
    }
    v
}

#[cfg(test)]
mod tests {
    use super::split_pieces;
    #[test]
    fn pieces_tile_and_respect_limit() {
        let s = "这是一个句子，".repeat(60) + &"长".repeat(300);
        let p = split_pieces(&s);
        assert_eq!(p.concat(), s);
        assert!(p.iter().all(|x| x.chars().count() <= super::MAX_PIECE));
    }
}
