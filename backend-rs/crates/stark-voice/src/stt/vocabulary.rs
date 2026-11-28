//! Parakeet's vocabulary bias, ported from Hex's `dictation/vocabulary.rs`.
//!
//! A term is a name Parakeet should spell right, optionally with "sounds
//! like" spellings a transcript may show instead. The engine resolves exact
//! spellings itself; terms and spellings steer the decoder's rescoring. The
//! list is read at the start of every run and every live stream, so a saved
//! change applies to the next dictation.

use serde::{Deserialize, Serialize};

/// Longest term or "sounds like" entry, in characters (Hex's limit).
pub const MAX_TERM_CHARS: usize = 64;
/// Most entries in the list (Hex's limit).
pub const MAX_ENTRIES: usize = 500;
/// Most "sounds like" spellings per entry (Hex's limit).
pub const MAX_SOUNDS_LIKE: usize = 16;
/// Most terms one transcription is biased toward: transcribe.cpp's cap,
/// which keeps the biasing a bounded cost per decode.
pub const MAX_BIASED_TERMS: usize = 64;

/// One vocabulary term and the spellings a transcript may show instead.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "SavedEntry")]
pub struct VocabularyEntry {
    pub term: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sounds_like: Vec<String>,
}

/// An entry as settings hold it: a plain term, or the term with its "sounds
/// like" spellings. Both forms load, like Hex's `settings.json`.
#[derive(Deserialize)]
#[serde(untagged)]
enum SavedEntry {
    Term(String),
    Entry {
        term: String,
        #[serde(default)]
        sounds_like: Vec<String>,
    },
}

impl From<SavedEntry> for VocabularyEntry {
    fn from(saved: SavedEntry) -> Self {
        match saved {
            SavedEntry::Term(term) => Self {
                term,
                sounds_like: Vec::new(),
            },
            SavedEntry::Entry { term, sounds_like } => Self { term, sounds_like },
        }
    }
}

/// How readily transcription swaps heard words for a term that sounds like
/// them (Hex's `VocabularyBoost`).
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VocabularyBoost {
    /// Transcripts are left as the model heard them.
    Off,
    Light,
    #[default]
    Normal,
    Strong,
}

impl VocabularyBoost {
    pub const ALL: [Self; 4] = [Self::Off, Self::Light, Self::Normal, Self::Strong];

    /// How much less likely than the heard words, in nats per term piece, a
    /// term may score and still replace them. These are Hex's exact values.
    pub fn nats(self) -> f32 {
        match self {
            Self::Off => 0.0,
            Self::Light => 2.0,
            Self::Normal => 2.5,
            Self::Strong => 4.0,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Light => "Light",
            Self::Normal => "Normal",
            Self::Strong => "Strong",
        }
    }
}

/// A vocabulary and its boost, as the app sends it with a dictation.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Vocabulary {
    pub entries: Vec<VocabularyEntry>,
    pub boost: VocabularyBoost,
}

/// What a Parakeet run reads: terms in priority order and the spellings
/// mapped back to them.
#[derive(Clone, Debug, PartialEq)]
pub struct BiasParts {
    pub terms: Vec<String>,
    pub sounds_like: Vec<(usize, String)>,
    pub boost: f32,
}

impl Vocabulary {
    /// Keep what Hex's list keeps: trimmed, unique terms in list order, at
    /// most 64 characters each, and up to 16 usable spellings per term.
    pub fn sanitized(mut self) -> Self {
        let mut entries: Vec<VocabularyEntry> = Vec::new();
        for entry in self.entries {
            let mut term = clean(&entry.term);
            if term.is_empty() {
                continue;
            }
            if term.chars().count() > MAX_TERM_CHARS {
                term = clean(&term.chars().take(MAX_TERM_CHARS).collect::<String>());
            }
            if term.is_empty() || contains(&entries, &term) {
                continue;
            }
            if entries.len() >= MAX_ENTRIES {
                break;
            }
            let mut sounds_like: Vec<String> = Vec::new();
            for spelling in entry.sounds_like {
                let spelling = clean(&spelling);
                if spelling.is_empty()
                    || spelling.chars().count() > MAX_TERM_CHARS
                    || same(&spelling, &term)
                    || sounds_like.iter().any(|kept| same(kept, &spelling))
                {
                    continue;
                }
                if sounds_like.len() >= MAX_SOUNDS_LIKE {
                    break;
                }
                sounds_like.push(spelling);
            }
            entries.push(VocabularyEntry { term, sounds_like });
        }
        self.entries = entries;
        self
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The terms and options a Parakeet run biases its transcript with.
    /// `None` while the list is empty or the boost is off, so an unused
    /// vocabulary adds no decoder work.
    pub fn bias_parts(&self) -> Option<BiasParts> {
        if self.boost == VocabularyBoost::Off {
            return None;
        }
        let entries = &self.entries[..self.entries.len().min(MAX_BIASED_TERMS)];
        if entries.is_empty() {
            return None;
        }
        let sounds_like = entries
            .iter()
            .enumerate()
            .flat_map(|(index, entry)| {
                entry
                    .sounds_like
                    .iter()
                    .map(move |spelling| (index, spelling.clone()))
            })
            .collect();
        Some(BiasParts {
            terms: entries.iter().map(|entry| entry.term.clone()).collect(),
            sounds_like,
            boost: self.boost.nats(),
        })
    }
}

/// Collapses whitespace, as Hex's `clean`.
pub fn clean(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn same(left: &str, right: &str) -> bool {
    left.to_lowercase() == right.to_lowercase()
}

/// Whether the list has an entry for `term`, in any case.
pub fn contains(list: &[VocabularyEntry], term: &str) -> bool {
    let term = clean(term);
    list.iter().any(|entry| same(&entry.term, &term))
}

/// Splits a comma-separated "sounds like" field into trimmed spellings.
pub fn parse_sounds_like(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(clean)
        .filter(|spelling| !spelling.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(term: &str, sounds_like: &[&str]) -> VocabularyEntry {
        VocabularyEntry {
            term: term.into(),
            sounds_like: sounds_like.iter().map(|&text| text.into()).collect(),
        }
    }

    fn vocabulary(entries: &[VocabularyEntry], boost: VocabularyBoost) -> Vocabulary {
        Vocabulary {
            entries: entries.to_vec(),
            boost,
        }
    }

    #[test]
    fn boost_uses_hexs_nats_and_names() {
        assert_eq!(VocabularyBoost::Off.nats(), 0.0);
        assert_eq!(VocabularyBoost::Light.nats(), 2.0);
        assert_eq!(VocabularyBoost::Normal.nats(), 2.5);
        assert_eq!(VocabularyBoost::Strong.nats(), 4.0);
        assert_eq!(VocabularyBoost::default(), VocabularyBoost::Normal);
        assert_eq!(
            serde_json::to_value(VocabularyBoost::Strong).unwrap(),
            serde_json::json!("strong")
        );
    }

    #[test]
    fn plain_terms_and_entries_both_load() {
        let parsed: Vocabulary = serde_json::from_str(
            r#"{"entries": ["Rune", {"term": "OpenCode", "sounds_like": ["open code"]}], "boost": "light"}"#,
        )
        .unwrap();
        assert_eq!(
            parsed.entries,
            [entry("Rune", &[]), entry("OpenCode", &["open code"]),]
        );
        assert_eq!(parsed.boost, VocabularyBoost::Light);
    }

    #[test]
    fn bias_keeps_the_first_terms_and_maps_their_spellings() {
        assert!(
            vocabulary(&[], VocabularyBoost::Normal)
                .bias_parts()
                .is_none()
        );
        let mut list = vec![
            entry("Cappuccino", &["capuchin", "cap a chino"]),
            entry("Rune", &[]),
        ];
        list.extend((0..70).map(|index| entry(&format!("term {index}"), &["x"])));
        let parts = vocabulary(&list, VocabularyBoost::Strong)
            .bias_parts()
            .unwrap();
        assert_eq!(parts.terms.len(), MAX_BIASED_TERMS);
        assert_eq!(parts.terms[..2], ["Cappuccino", "Rune"]);
        assert_eq!(parts.boost, 4.0);
        assert_eq!(
            parts.sounds_like[..2],
            [(0, "capuchin".into()), (0, "cap a chino".into())]
        );
        assert_eq!(parts.sounds_like[2], (2, "x".into()));
        assert!(
            parts
                .sounds_like
                .iter()
                .all(|(index, _)| *index < MAX_BIASED_TERMS)
        );
    }

    #[test]
    fn the_boost_off_turns_biasing_off() {
        let list = vocabulary(&[entry("Rune", &[])], VocabularyBoost::Off);
        assert!(list.bias_parts().is_none());
    }

    #[test]
    fn sanitizing_trims_dedupes_and_clamps_like_hex() {
        let list = vocabulary(
            &[
                entry(
                    "  Cappuccino ",
                    &[" capuchin ", "capuchin", "Cappuccino", ""],
                ),
                entry("cappuccino", &["cap a chino"]),
                entry("", &["next"]),
                entry("Rune", &["roon"]),
            ],
            VocabularyBoost::Normal,
        )
        .sanitized();
        assert_eq!(
            list.entries,
            [entry("Cappuccino", &["capuchin"]), entry("Rune", &["roon"]),],
            "a case-only duplicate keeps the first entry"
        );

        let long = "x".repeat(MAX_TERM_CHARS + 10);
        let clamped = vocabulary(&[entry(&long, &[])], VocabularyBoost::Normal).sanitized();
        assert_eq!(clamped.entries[0].term.chars().count(), MAX_TERM_CHARS);

        let spellings: Vec<String> = (0..20).map(|index| format!("spelling {index}")).collect();
        let spellings: Vec<&str> = spellings.iter().map(String::as_str).collect();
        let capped = vocabulary(&[entry("Many", &spellings)], VocabularyBoost::Normal).sanitized();
        assert_eq!(capped.entries[0].sounds_like.len(), MAX_SOUNDS_LIKE);
    }

    #[test]
    fn sanitizing_keeps_terms_unique_in_list_order_within_the_cap() {
        let many: Vec<VocabularyEntry> = (0..MAX_ENTRIES + 5)
            .map(|index| entry(&format!("term {index}"), &[]))
            .collect();
        let list = vocabulary(&many, VocabularyBoost::Normal).sanitized();
        assert_eq!(list.entries.len(), MAX_ENTRIES);
        assert_eq!(list.entries[0].term, "term 0");
    }

    #[test]
    fn sounds_like_splits_on_commas() {
        assert_eq!(
            parse_sounds_like(" capuchin,, cap  a chino ,"),
            ["capuchin", "cap a chino"]
        );
        assert!(parse_sounds_like(" , ").is_empty());
    }
}
