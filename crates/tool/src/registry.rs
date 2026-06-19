use std::collections::HashMap;
use std::sync::Arc;
use crate::traits::{Tool, ToolClass, ToolDef};

pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
    names: Vec<String>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
            names: Vec::new(),
        }
    }

    pub fn register<T: Tool + 'static>(&mut self, tool: T) -> &mut Self {
        let name = tool.name().to_string();
        self.names.push(name.clone());
        self.tools.insert(name, Arc::new(tool));
        self
    }

    pub fn get(&self, name: &str) -> Option<&Arc<dyn Tool>> {
        self.tools.get(name)
    }

    pub fn list(&self) -> Vec<(String, String, ToolClass)> {
        self.names
            .iter()
            .filter_map(|name| {
                self.tools.get(name).map(|t| {
                    (name.clone(), t.description().to_string(), t.class())
                })
            })
            .collect()
    }

    pub fn get_definitions(&self) -> Vec<ToolDef> {
        self.names
            .iter()
            .filter_map(|name| {
                self.tools.get(name).map(|t| ToolDef {
                    name: t.name().to_string(),
                    description: t.description().to_string(),
                    parameters: t.input_schema(),
                })
            })
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    pub fn len(&self) -> usize {
        self.tools.len()
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}
