//! Reader for the HUP-S3.6 `skills.lock` (generated in citrate-core by
//! `scripts/skills-lock.mjs`). The corpus build ships a third-party skill only
//! when the lock admits it, and only the files the lock pins by sha256.

use serde::Deserialize;

use crate::CorpusError;

#[derive(Debug, Clone, Deserialize)]
pub struct SkillsLock {
    pub version: u32,
    #[serde(rename = "source", default)]
    pub sources: Vec<LockSource>,
    #[serde(rename = "skill", default)]
    pub skills: Vec<LockSkill>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LockSource {
    pub label: String,
    pub upstream: String,
    pub commit: String,
    pub license: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LockRef {
    pub path: String,
    pub sha256: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LockSkill {
    pub name: String,
    pub source: String,
    pub commit: String,
    pub path: String,
    pub verdict: String,
    #[serde(default)]
    pub skill_md_sha256: Option<String>,
    #[serde(default)]
    pub refs: Vec<LockRef>,
}

/// What the lock's verdict means for the bundle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ship {
    /// Ship `SKILL.md` and the pinned refs (scripts are never shipped: the lock
    /// lists only non-executable refs, whatever the verdict).
    Text,
    /// Do not ship.
    No,
}

impl LockSkill {
    pub fn ship(&self) -> Result<Ship, CorpusError> {
        match self.verdict.as_str() {
            "include-as-is" | "include-with-scripts-stripped" | "convert-script-to-capsule" => {
                Ok(Ship::Text)
            }
            "exclude" => Ok(Ship::No),
            other => Err(CorpusError::Lock(format!(
                "skill {:?}: unknown verdict {other:?} (refusing to guess)",
                self.name
            ))),
        }
    }
}

impl SkillsLock {
    pub fn from_toml(text: &str) -> Result<Self, CorpusError> {
        let lock: SkillsLock =
            toml::from_str(text).map_err(|e| CorpusError::Lock(e.to_string()))?;
        if lock.version != 1 {
            return Err(CorpusError::Lock(format!(
                "unsupported skills.lock version {}",
                lock.version
            )));
        }
        Ok(lock)
    }

    pub fn source(&self, label: &str) -> Option<&LockSource> {
        self.sources.iter().find(|s| s.label == label)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_verdict_fails_closed() {
        let s = LockSkill {
            name: "x".into(),
            source: "s".into(),
            commit: "c".into(),
            path: "p".into(),
            verdict: "include-maybe".into(),
            skill_md_sha256: None,
            refs: vec![],
        };
        assert!(s.ship().is_err());
    }
}
