//! The binary evidence log — the production on-disk format.
//!
//! JSONL is wonderful for debugging but pays for it twice in production:
//! every record repeats its field names (`"entity":`, `"observed_at":`, …
//! ~70 bytes of pure syntax) and every number is re-parsed from ASCII on
//! replay. This format keeps the same append-only, one-record-per-entry
//! shape but drops both costs: length-prefixed strings, little-endian
//! binary numbers, no field names.
//!
//! ```text
//! file   := MAGIC record*
//! MAGIC  := "NSCLOG01"              (8 bytes)
//! record := u8 tag                  (0=interval, 1=value, 2=not_value)
//!           str entity              (u16 len + utf8)
//!           str source
//!           i64 observed_at         (little-endian)
//!           str slot
//!           tag==0 -> f64 lo, f64 hi
//!           else   -> str value
//! ```
//!
//! Recovery is WAL-style: a torn trailing record (a crash mid-append,
//! before the fsync that would have acknowledged it) is dropped, and the
//! number of ignored bytes is returned so the caller can surface it.
//! Readable JSON is never lost — `nescio export` reconstructs it, and
//! `nescio import` goes the other way.

use crate::error::{Error, Result};
use crate::model::evidence::{Claim, EvidenceRecord};

pub const MAGIC: &[u8; 8] = b"NSCLOG01";
pub const LOG_BIN: &str = "log.bin";

/// A fresh file's contents: just the magic header.
pub fn header() -> Vec<u8> {
    MAGIC.to_vec()
}

pub fn encode(rec: &EvidenceRecord, out: &mut Vec<u8>) {
    match &rec.claim {
        Claim::Interval { slot, lo, hi } => {
            out.push(0);
            common(out, rec, slot);
            out.extend_from_slice(&lo.to_le_bytes());
            out.extend_from_slice(&hi.to_le_bytes());
        }
        Claim::Value { slot, value } => {
            out.push(1);
            common(out, rec, slot);
            put_str(out, value);
        }
        Claim::NotValue { slot, value } => {
            out.push(2);
            common(out, rec, slot);
            put_str(out, value);
        }
    }
}

fn common(out: &mut Vec<u8>, rec: &EvidenceRecord, slot: &str) {
    put_str(out, &rec.entity);
    put_str(out, &rec.source);
    out.extend_from_slice(&rec.observed_at.to_le_bytes());
    put_str(out, slot);
}

fn put_str(out: &mut Vec<u8>, s: &str) {
    let len = s.len().min(u16::MAX as usize);
    out.extend_from_slice(&(len as u16).to_le_bytes());
    out.extend_from_slice(&s.as_bytes()[..len]);
}

/// Decode a whole log. Returns the records plus the count of trailing bytes
/// ignored (a torn tail from a crash). Errors only on a bad magic header.
pub fn decode(bytes: &[u8]) -> Result<(Vec<EvidenceRecord>, usize)> {
    if bytes.is_empty() {
        return Ok((Vec::new(), 0));
    }
    if bytes.len() < MAGIC.len() || &bytes[..MAGIC.len()] != MAGIC {
        return Err(Error::Invalid(
            "log.bin: bad magic header (not a nescioDB binary log)".into(),
        ));
    }
    let mut cur = Cursor {
        b: bytes,
        pos: MAGIC.len(),
    };
    let mut out = Vec::new();
    loop {
        let start = cur.pos;
        match cur.record() {
            Some(rec) => out.push(rec),
            None => {
                // Clean EOF or a torn/invalid trailing record.
                return Ok((out, bytes.len() - start));
            }
        }
    }
}

struct Cursor<'a> {
    b: &'a [u8],
    pos: usize,
}

impl Cursor<'_> {
    fn take(&mut self, n: usize) -> Option<&[u8]> {
        let end = self.pos.checked_add(n)?;
        if end > self.b.len() {
            return None;
        }
        let s = &self.b[self.pos..end];
        self.pos = end;
        Some(s)
    }

    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }

    fn u16(&mut self) -> Option<usize> {
        let s = self.take(2)?;
        Some(u16::from_le_bytes([s[0], s[1]]) as usize)
    }

    fn i64(&mut self) -> Option<i64> {
        let s = self.take(8)?;
        Some(i64::from_le_bytes(s.try_into().unwrap()))
    }

    fn f64(&mut self) -> Option<f64> {
        let s = self.take(8)?;
        Some(f64::from_le_bytes(s.try_into().unwrap()))
    }

    fn string(&mut self) -> Option<String> {
        let n = self.u16()?;
        let s = self.take(n)?;
        Some(String::from_utf8_lossy(s).into_owned())
    }

    fn record(&mut self) -> Option<EvidenceRecord> {
        let tag = self.u8()?;
        let entity = self.string()?;
        let source = self.string()?;
        let observed_at = self.i64()?;
        let slot = self.string()?;
        let claim = match tag {
            0 => {
                let lo = self.f64()?;
                let hi = self.f64()?;
                Claim::Interval { slot, lo, hi }
            }
            1 => Claim::Value {
                slot,
                value: self.string()?,
            },
            2 => Claim::NotValue {
                slot,
                value: self.string()?,
            },
            _ => return None, // unknown tag: stop (treat the rest as a torn tail)
        };
        Some(EvidenceRecord {
            entity,
            claim,
            source,
            observed_at,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<EvidenceRecord> {
        vec![
            EvidenceRecord {
                entity: "villa_1".into(),
                claim: Claim::Interval {
                    slot: "price".into(),
                    lo: 900_000.0,
                    hi: 1_000_000.0,
                },
                source: "broker".into(),
                observed_at: 1_000_000,
            },
            EvidenceRecord {
                entity: "villa_1".into(),
                claim: Claim::Value {
                    slot: "condition".into(),
                    value: "original".into(),
                },
                source: "notary".into(),
                observed_at: 2_000_000,
            },
        ]
    }

    #[test]
    fn roundtrip() {
        let recs = sample();
        let mut buf = header();
        for r in &recs {
            encode(r, &mut buf);
        }
        let (out, trailing) = decode(&buf).unwrap();
        assert_eq!(trailing, 0);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].entity, "villa_1");
        assert!(matches!(out[0].claim, Claim::Interval { .. }));
        assert!(matches!(&out[1].claim, Claim::Value { value, .. } if value == "original"));
        assert_eq!(out[1].observed_at, 2_000_000);
    }

    #[test]
    fn torn_tail_is_dropped_not_fatal() {
        let recs = sample();
        let mut buf = header();
        for r in &recs {
            encode(r, &mut buf);
        }
        // Chop the last few bytes: simulate a crash mid-append.
        buf.truncate(buf.len() - 5);
        let (out, trailing) = decode(&buf).unwrap();
        assert_eq!(out.len(), 1); // first record survives, torn second dropped
        assert!(trailing > 0);
    }

    #[test]
    fn bad_magic_errors() {
        assert!(decode(b"not-a-log-at-all-really").is_err());
    }

    #[test]
    fn empty_is_ok() {
        assert_eq!(decode(&[]).unwrap().0.len(), 0);
        assert_eq!(decode(&header()).unwrap().0.len(), 0);
    }

    #[test]
    fn much_smaller_than_json() {
        let recs = sample();
        let mut bin = header();
        let mut json = 0usize;
        for r in &recs {
            encode(r, &mut bin);
            json += serde_json::to_string(r).unwrap().len() + 1;
        }
        assert!(
            bin.len() * 2 < json,
            "binary {} vs json {}",
            bin.len(),
            json
        );
    }
}
