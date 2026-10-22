use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSpecFile {
    pub contract_version: u32,
    pub source: String,
    pub tool_count: usize,
    pub tools: Vec<ToolSpec>,
    pub scope_map: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub missing_docs: Vec<String>,
    #[serde(default)]
    pub orphan_docs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
    #[serde(default = "default_true")]
    pub exposed: bool,
    #[serde(default)]
    pub required: Vec<String>,
    #[serde(default)]
    pub doc: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum ToolSpecError {
    #[error("contract_version must be at least 1")]
    InvalidVersion,
    #[error("tool_count {declared} does not match {actual} tools")]
    CountMismatch { declared: usize, actual: usize },
    #[error("duplicate tool name: {0}")]
    DuplicateTool(String),
    #[error("tool has an empty name")]
    EmptyName,
    #[error("tool {0} has an empty description")]
    EmptyDescription(String),
    #[error("tool {0} parameters must be a JSON object")]
    InvalidParameters(String),
    #[error("tool {0} parameters.properties must be a JSON object")]
    InvalidProperties(String),
    #[error("scope {scope} references unknown tool {tool}")]
    UnknownScopeTool { scope: String, tool: String },
    #[error("tool {tool} requires {arg}, but it is not in parameters.properties")]
    MissingRequiredProperty { tool: String, arg: String },
}

#[derive(Debug, thiserror::Error)]
pub enum ToolSpecLoadError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Validation(#[from] ToolSpecError),
}

impl ToolSpecFile {
    pub fn from_json_str(raw: &str) -> Result<Self, ToolSpecLoadError> {
        let file: Self = serde_json::from_str(raw)?;
        file.validate()?;
        Ok(file)
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Self, ToolSpecLoadError> {
        Self::from_json_str(&std::fs::read_to_string(path)?)
    }

    pub fn validate(&self) -> Result<(), ToolSpecError> {
        if self.contract_version == 0 {
            return Err(ToolSpecError::InvalidVersion);
        }
        if self.tool_count != self.tools.len() {
            return Err(ToolSpecError::CountMismatch {
                declared: self.tool_count,
                actual: self.tools.len(),
            });
        }

        let mut names = BTreeMap::new();
        for tool in &self.tools {
            if tool.name.trim().is_empty() {
                return Err(ToolSpecError::EmptyName);
            }
            if names.insert(tool.name.as_str(), ()).is_some() {
                return Err(ToolSpecError::DuplicateTool(tool.name.clone()));
            }
            if tool.description.trim().is_empty() {
                return Err(ToolSpecError::EmptyDescription(tool.name.clone()));
            }
            let parameters = tool
                .parameters
                .as_object()
                .ok_or_else(|| ToolSpecError::InvalidParameters(tool.name.clone()))?;
            let properties = match parameters.get("properties") {
                Some(value) => Some(
                    value
                        .as_object()
                        .ok_or_else(|| ToolSpecError::InvalidProperties(tool.name.clone()))?,
                ),
                None => None,
            };
            for arg in &tool.required {
                let present = properties.is_some_and(|props| props.contains_key(arg));
                if !present {
                    return Err(ToolSpecError::MissingRequiredProperty {
                        tool: tool.name.clone(),
                        arg: arg.clone(),
                    });
                }
            }
        }

        for (scope, tools) in &self.scope_map {
            for tool in tools {
                if !names.contains_key(tool.as_str()) {
                    return Err(ToolSpecError::UnknownScopeTool {
                        scope: scope.clone(),
                        tool: tool.clone(),
                    });
                }
            }
        }

        Ok(())
    }

    pub fn exposed_tools(&self) -> impl Iterator<Item = &ToolSpec> {
        self.tools.iter().filter(|tool| tool.exposed)
    }

    pub fn tool(&self, name: &str) -> Option<&ToolSpec> {
        self.tools.iter().find(|tool| tool.name == name)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::PathBuf;

    use super::*;

    fn fixture() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../specs/tool-specs.json")
    }

    #[test]
    fn frozen_contract_loads_and_validates() {
        let file = ToolSpecFile::load(fixture()).expect("tool contract");

        assert_eq!(file.contract_version, 1);
        assert_eq!(file.tool_count, 42);
        assert_eq!(file.tools.len(), 42);
        assert_eq!(file.scope_map.len(), 8);
        assert_eq!(file.exposed_tools().count(), 36);

        let hidden: BTreeSet<&str> = file
            .tools
            .iter()
            .filter(|tool| !tool.exposed)
            .map(|tool| tool.name.as_str())
            .collect();
        assert_eq!(
            hidden,
            BTreeSet::from([
                "browser_navigate",
                "browser_extract",
                "browser_screenshot",
                "browser_close",
                "update_feature",
                "update_project_meta",
            ])
        );

        assert_eq!(
            file.missing_docs,
            vec!["update_feature", "update_project_meta", "files_changed"]
        );
        assert_eq!(file.orphan_docs, vec!["browser_click", "browser_fill"]);
        assert!(file.tool("propose_plan").is_none());

        assert_eq!(file.scope_map["developer"].len(), 7);
        assert_eq!(file.scope_map["general"].len(), 10);
        assert_eq!(file.scope_map["house"].len(), 9);
        assert_eq!(file.scope_map["dfd"].len(), 7);
        assert_eq!(file.scope_map["orchestrator"].len(), 14);
        assert_eq!(file.scope_map["devops"].len(), 6);
        assert_eq!(file.scope_map["explore"].len(), 3);
        assert_eq!(file.scope_map["ui"].len(), 7);

        assert_eq!(
            file.tool("read_file").unwrap().required,
            Vec::<String>::new()
        );
        assert_eq!(
            file.tool("read_pdf").unwrap().required,
            vec!["file_path".to_string()]
        );
    }

    #[test]
    fn json_round_trips_exactly() {
        let raw = std::fs::read_to_string(fixture()).expect("contract file");
        let file = ToolSpecFile::from_json_str(&raw).expect("contract parses");
        let generated = serde_json::to_value(&file).expect("contract serializes");
        let original: serde_json::Value = serde_json::from_str(&raw).expect("fixture json");
        assert_eq!(generated, original);
    }
}
