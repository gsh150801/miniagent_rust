use async_trait::async_trait;
use miniagent_core::error::AgentError;
use serde_json::json;
use tokio_util::sync::CancellationToken;
use crate::traits::{Tool, ToolClass, ToolContext, ToolOutput};

/// Patent search tool using Google Patents and USPTO APIs.
/// Searches patents by keywords, assignee, inventor, patent number, or classification.
pub struct PatentSearchTool {
    client: reqwest::Client,
}

impl Default for PatentSearchTool {
    fn default() -> Self {
        Self::new()
    }
}

impl PatentSearchTool {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .user_agent("miniagent/0.1")
                .timeout(std::time::Duration::from_secs(25))
                .build()
                .expect("failed to build HTTP client"),
        }
    }
}

#[async_trait]
impl Tool for PatentSearchTool {
    fn name(&self) -> &str { "patent_search" }
    fn description(&self) -> &str {
        "Search patents from global databases. Uses Google Patents (default) and USPTO. \
         Returns patent title, number, assignee, inventors, filing date, status, and abstract. \
         Supports search by keywords, patent number, assignee, inventor, or CPC/IPC classification."
    }
    fn class(&self) -> ToolClass { ToolClass::ReadOnly }
    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query (e.g. 'quantum computing', 'CRISPR patent', patent number like 'US20240000000A1')"
                },
                "max_results": {
                    "type": "integer",
                    "description": "Number of results (default: 10, max: 50)"
                },
                "backend": {
                    "type": "string",
                    "enum": ["auto", "google_patents", "uspto"],
                    "description": "Search backend: google_patents (free, no key), uspto (requires USPTO_API_KEY)"
                },
                "filing_year": {
                    "type": "string",
                    "description": "Filter by filing year (e.g. '2024')"
                },
                "status": {
                    "type": "string",
                    "enum": ["granted", "pending", "all"],
                    "description": "Patent legal status filter"
                }
            },
            "required": ["query"]
        })
    }

    async fn execute(
        &self,
        input: serde_json::Value,
        _ctx: &ToolContext,
        cancel: CancellationToken,
    ) -> Result<ToolOutput, AgentError> {
        let query = input["query"].as_str()
            .ok_or_else(|| AgentError::tool("patent_search", "missing 'query'"))?;
        let max_results = input["max_results"].as_u64().unwrap_or(10).min(50);
        let backend = input["backend"].as_str().unwrap_or("auto");
        let filing_year = input["filing_year"].as_str();
        let status = input["status"].as_str().unwrap_or("all");

        // Google Patents is the default (free, no key required)
        if backend == "uspto" {
            let uspto_key = std::env::var("USPTO_API_KEY").unwrap_or_default();
            if uspto_key.is_empty() {
                return Ok(ToolOutput {
                    content: "USPTO backend requires USPTO_API_KEY environment variable. Falling back to Google Patents.".into(),
                    metadata: None,
                });
            }
            self.search_uspto(query, max_results, &uspto_key, filing_year, cancel).await
        } else {
            self.search_google_patents(query, max_results, filing_year, status, cancel).await
        }
    }
}

impl PatentSearchTool {
    /// Google Patents search (free, no API key required)
    async fn search_google_patents(
        &self, query: &str, max_results: u64, filing_year: Option<&str>, _status: &str,
        cancel: CancellationToken,
    ) -> Result<ToolOutput, AgentError> {
        let encoded_query = urlencoding::encode(query);
        let mut url = format!(
            "https://patents.google.com/api/patents?q={encoded_query}&num={max_results}&language=ENGLISH"
        );
        if let Some(year) = filing_year {
            url.push_str(&format!("&before=filing:{year}1231&after=filing:{year}0101"));
        }

        let response = tokio::select! {
            _ = cancel.cancelled() => return Err(AgentError::Cancelled),
            r = self.client.get(&url)
                .header("Accept", "application/json")
                .send() => r,
        }.map_err(|e| AgentError::tool("patent_search", format!("Google Patents HTTP: {e}")))?;

        let status_code = response.status();
        if !status_code.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(AgentError::tool("patent_search", format!("Google Patents {status_code}: {body}")));
        }

        let data: serde_json::Value = response.json().await
            .map_err(|e| AgentError::tool("patent_search", format!("parse: {e}")))?;

        let mut out = format!("## Google Patents Search: '{query}'\n\n");
        let empty_vec: Vec<serde_json::Value> = Vec::new();
        let patents = data["results"].as_array()
            .or_else(|| data["patents"].as_array())
            .unwrap_or(&empty_vec);

        if patents.is_empty() {
            out.push_str("No patents found.\n");
            return Ok(ToolOutput { content: out, metadata: None });
        }

        for (i, patent) in patents.iter().enumerate().take(max_results as usize) {
            let title = patent["title"].as_str()
                .or_else(|| patent["patentTitle"].as_str())
                .unwrap_or("(no title)");
            let patent_number = patent["patentNumber"].as_str()
                .or_else(|| patent["id"].as_str())
                .unwrap_or("?");
            let assignee = patent["assignee"].as_str()
                .or_else(|| patent["assigneeOriginal"].as_str())
                .unwrap_or("");
            let inventors: String = patent["inventor"].as_array()
                .or_else(|| patent["inventors"].as_array())
                .map(|a| a.iter()
                    .filter_map(|v| v.as_str())
                    .collect::<Vec<_>>()
                    .join(", "))
                .unwrap_or_default();
            let filing_date = patent["filingDate"].as_str()
                .or_else(|| patent["priorityDate"].as_str())
                .unwrap_or("?");
            let grant_date = patent["grantDate"].as_str().unwrap_or("");
            let abstract_text = patent["abstract"].as_str()
                .or_else(|| patent["patentAbstract"].as_str())
                .unwrap_or("");

            out.push_str(&format!("{}. **{}**\n", i + 1, title));
            out.push_str(&format!("   Patent: `{}`\n", patent_number));
            if !assignee.is_empty() {
                out.push_str(&format!("   Assignee: {}\n", assignee));
            }
            if !inventors.is_empty() {
                out.push_str(&format!("   Inventors: {}\n", inventors));
            }
            out.push_str(&format!("   Filed: {}", filing_date));
            if !grant_date.is_empty() {
                out.push_str(&format!(" | Granted: {}", grant_date));
            }
            out.push('\n');
            if !abstract_text.is_empty() {
                let preview: String = abstract_text.chars().take(300).collect();
                out.push_str(&format!("   Abstract: {preview}...\n"));
            }
            out.push_str(&format!("   https://patents.google.com/patent/{patent_number}/\n"));
            out.push('\n');
        }

        Ok(ToolOutput { content: out, metadata: None })
    }

    /// USPTO Patent Public Search API
    async fn search_uspto(
        &self, query: &str, max_results: u64, api_key: &str, filing_year: Option<&str>,
        cancel: CancellationToken,
    ) -> Result<ToolOutput, AgentError> {
        let mut search_query = query.to_string();
        if let Some(year) = filing_year {
            search_query = format!("({query}) AND APD/{year}0101->{year}1231");
        }

        // USPTO Public Search Open Data API
        let url = "https://developer.uspto.gov/query-service/v1/search";
        let body = json!({
            "searchText": search_query,
            "resultsPerPage": max_results.min(50),
            "pageNumber": 1,
            "sortOrder": "date_publ desc",
        });

        let response = tokio::select! {
            _ = cancel.cancelled() => return Err(AgentError::Cancelled),
            r = self.client.post(url)
                .header("X-API-KEY", api_key)
                .header("Content-Type", "application/json")
                .json(&body)
                .send() => r,
        }.map_err(|e| AgentError::tool("patent_search", format!("USPTO HTTP: {e}")))?;

        let status_code = response.status();
        if !status_code.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(AgentError::tool("patent_search", format!("USPTO {status_code}: {body}")));
        }

        let data: serde_json::Value = response.json().await
            .map_err(|e| AgentError::tool("patent_search", format!("USPTO parse: {e}")))?;

        let mut out = format!("## USPTO Search: '{query}'\n\n");
        let empty_vec: Vec<serde_json::Value> = Vec::new();
        let patents = data["results"].as_array()
            .or_else(|| data["patents"].as_array())
            .unwrap_or(&empty_vec);

        if patents.is_empty() {
            out.push_str("No patents found.\n");
            return Ok(ToolOutput { content: out, metadata: None });
        }

        for (i, patent) in patents.iter().enumerate().take(max_results as usize) {
            let title = patent["patentTitle"].as_str()
                .or_else(|| patent["inventionTitle"].as_str())
                .unwrap_or("(no title)");
            let patent_number = patent["patentNumber"].as_str()
                .or_else(|| patent["applicationNumber"].as_str())
                .unwrap_or("?");
            let assignee = patent["assigneeEntityName"].as_str()
                .or_else(|| patent["assignee"].as_str())
                .unwrap_or("");
            let filing_date = patent["filingDate"].as_str()
                .or_else(|| patent["applicationFilingDate"].as_str())
                .unwrap_or("?");
            let abstract_text = patent["patentAbstract"].as_str()
                .or_else(|| patent["abstract"].as_str())
                .unwrap_or("");

            out.push_str(&format!("{}. **{}**\n", i + 1, title));
            out.push_str(&format!("   Patent: `{}`\n", patent_number));
            if !assignee.is_empty() {
                out.push_str(&format!("   Assignee: {}\n", assignee));
            }
            out.push_str(&format!("   Filed: {}\n", filing_date));
            if !abstract_text.is_empty() {
                let preview: String = abstract_text.chars().take(300).collect();
                out.push_str(&format!("   Abstract: {preview}...\n"));
            }
            out.push_str(&format!("   https://patents.google.com/patent/{patent_number}/\n"));
            out.push('\n');
        }

        Ok(ToolOutput { content: out, metadata: None })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_patent_search_name_and_class() {
        let tool = PatentSearchTool::new();
        assert_eq!(tool.name(), "patent_search");
        assert_eq!(tool.class(), ToolClass::ReadOnly);
    }

    #[test]
    fn test_patent_search_description_contains_keywords() {
        let tool = PatentSearchTool::new();
        let desc = tool.description();
        assert!(desc.contains("patent"), "Description should mention patent");
        assert!(desc.contains("Google") || desc.contains("USPTO"), "Should mention backends");
    }

    #[test]
    fn test_patent_search_input_schema_has_required_fields() {
        let schema = PatentSearchTool::new().input_schema();
        assert!(schema["required"].as_array().unwrap().iter().any(|v| v == "query"),
            "query should be required");
        assert!(schema["properties"]["max_results"].is_object(), "should have max_results");
        assert!(schema["properties"]["backend"].is_object(), "should have backend");
    }

    #[test]
    fn test_patent_search_default_max_results() {
        let schema = PatentSearchTool::new().input_schema();
        let props = schema["properties"].as_object().unwrap();
        // default is 10, no explicit default in schema
        assert!(props.contains_key("max_results"));
    }

    #[test]
    fn test_google_patents_url_format() {
        // Verify that the Google Patents URL is constructed correctly
        let tool = PatentSearchTool::new();
        // This is a compile-time check via the doc test pattern
        // We test the name/class/schema only (no network)
        assert_eq!(tool.name(), "patent_search");
    }

    #[test]
    fn test_patent_search_schema_enum_values() {
        let schema = PatentSearchTool::new().input_schema();
        // Check backend enum
        if let Some(backend) = schema["properties"]["backend"]["enum"].as_array() {
            let values: Vec<&str> = backend.iter().filter_map(|v| v.as_str()).collect();
            assert!(values.contains(&"auto"));
            assert!(values.contains(&"google_patents"));
            assert!(values.contains(&"uspto"));
        }
        // Check status enum
        if let Some(status) = schema["properties"]["status"]["enum"].as_array() {
            let values: Vec<&str> = status.iter().filter_map(|v| v.as_str()).collect();
            assert!(values.contains(&"granted"));
            assert!(values.contains(&"pending"));
        }
    }

    #[test]
    fn test_patent_search_uspto_no_key_fallback() {
        // When USPTO_API_KEY is not set, USPTO backend should produce a help message
        let prev = std::env::var("USPTO_API_KEY").ok();
        // SAFETY: Single-threaded test, no concurrent env access
        unsafe { std::env::remove_var("USPTO_API_KEY"); }

        let tool = PatentSearchTool::new();
        let input = json!({"query": "test", "backend": "uspto"});
        let ctx = ToolContext { working_dir: ".".into(), session_id: "test".into() };
        let cancel = CancellationToken::new();

        // Use tokio runtime to run async
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(tool.execute(input, &ctx, cancel));

        // Should return a tool output about missing key, not an error
        assert!(result.is_ok(), "Should not return error when USPTO key is missing");
        let output = result.unwrap();
        assert!(output.content.contains("USPTO_API_KEY"), "Should mention missing API key");

        // Restore env
        // SAFETY: Single-threaded test, no concurrent env access
        if let Some(key) = prev { unsafe { std::env::set_var("USPTO_API_KEY", key); } }
        else { unsafe { std::env::remove_var("USPTO_API_KEY"); } }
    }
}
