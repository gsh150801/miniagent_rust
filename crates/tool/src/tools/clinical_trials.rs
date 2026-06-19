use async_trait::async_trait;
use miniagent_core::error::AgentError;
use serde_json::json;
use tokio_util::sync::CancellationToken;
use crate::traits::{Tool, ToolClass, ToolContext, ToolOutput};

/// Clinical trials search tool using ClinicalTrials.gov API v2 (free, no key required).
/// Searches interventional and observational studies globally.
pub struct ClinicalTrialsTool {
    client: reqwest::Client,
}

impl Default for ClinicalTrialsTool {
    fn default() -> Self {
        Self::new()
    }
}

impl ClinicalTrialsTool {
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
impl Tool for ClinicalTrialsTool {
    fn name(&self) -> &str { "clinical_trials_search" }
    fn description(&self) -> &str {
        "Search clinical trials from ClinicalTrials.gov (free, no API key). \
         Returns study title, NCT number, status, conditions, interventions, phase, \
         sponsor, enrollment, locations, and brief summary. \
         Supports filtering by recruitment status, phase, study type, and date range."
    }
    fn class(&self) -> ToolClass { ToolClass::ReadOnly }
    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query: condition, drug, intervention, or NCT number"
                },
                "max_results": {
                    "type": "integer",
                    "description": "Number of results (default: 10, max: 50)"
                },
                "status": {
                    "type": "string",
                    "enum": ["all", "recruiting", "active_not_recruiting", "completed", "terminated", "withdrawn"],
                    "description": "Recruitment status filter"
                },
                "phase": {
                    "type": "string",
                    "enum": ["all", "early_phase_1", "phase_1", "phase_2", "phase_3", "phase_4", "not_applicable"],
                    "description": "Trial phase filter"
                },
                "study_type": {
                    "type": "string",
                    "enum": ["all", "interventional", "observational", "expanded_access"],
                    "description": "Type of study"
                },
                "min_year": {
                    "type": "string",
                    "description": "Minimum start year (e.g. '2023')"
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
            .ok_or_else(|| AgentError::tool("clinical_trials", "missing 'query'"))?;
        let max_results = input["max_results"].as_u64().unwrap_or(10).min(50);
        let status = input["status"].as_str().unwrap_or("all");
        let phase = input["phase"].as_str().unwrap_or("all");
        let study_type = input["study_type"].as_str().unwrap_or("all");
        let min_year = input["min_year"].as_str();

        self.search_ct_gov(query, max_results, status, phase, study_type, min_year, cancel).await
    }
}

impl ClinicalTrialsTool {
    async fn search_ct_gov(
        &self, query: &str, max_results: u64, status: &str, phase: &str, study_type: &str,
        min_year: Option<&str>, cancel: CancellationToken,
    ) -> Result<ToolOutput, AgentError> {
        // ClinicalTrials.gov API v2 — no API key required
        // Use the "format=json" endpoint
        let encoded_query = urlencoding::encode(query);
        let mut url = format!(
            "https://clinicaltrials.gov/api/v2/studies?query.term={encoded_query}&pageSize={}&format=json",
            max_results.min(50)
        );

        // Filter by recruitment status
        match status {
            "recruiting" => url.push_str("&filter.overallStatus=RECRUITING"),
            "active_not_recruiting" => url.push_str("&filter.overallStatus=ACTIVE_NOT_RECRUITING"),
            "completed" => url.push_str("&filter.overallStatus=COMPLETED"),
            "terminated" => url.push_str("&filter.overallStatus=TERMINATED"),
            "withdrawn" => url.push_str("&filter.overallStatus=WITHDRAWN"),
            _ => {}
        }

        // Filter by phase
        match phase {
            "early_phase_1" => url.push_str("&filter.phase=EARLY_PHASE1"),
            "phase_1" => url.push_str("&filter.phase=PHASE1"),
            "phase_2" => url.push_str("&filter.phase=PHASE2"),
            "phase_3" => url.push_str("&filter.phase=PHASE3"),
            "phase_4" => url.push_str("&filter.phase=PHASE4"),
            "not_applicable" => url.push_str("&filter.phase=NA"),
            _ => {}
        }

        // Filter by study type
        match study_type {
            "interventional" => url.push_str("&filter.studyType=INTERVENTIONAL"),
            "observational" => url.push_str("&filter.studyType=OBSERVATIONAL"),
            "expanded_access" => url.push_str("&filter.studyType=EXPANDED_ACCESS"),
            _ => {}
        }

        // Filter by start date
        if let Some(year) = min_year {
            url.push_str(&format!("&filter.startDate=MIN:{year}-01-01"));
        }

        let response = tokio::select! {
            _ = cancel.cancelled() => return Err(AgentError::Cancelled),
            r = self.client.get(&url).send() => r,
        }.map_err(|e| AgentError::tool("clinical_trials", format!("HTTP: {e}")))?;

        let status_code = response.status();
        if !status_code.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(AgentError::tool("clinical_trials", format!("{status_code}: {body}")));
        }

        let data: serde_json::Value = response.json().await
            .map_err(|e| AgentError::tool("clinical_trials", format!("parse: {e}")))?;

        let mut out = format!("## ClinicalTrials.gov: '{query}'\n\n");

        let empty_vec: Vec<serde_json::Value> = Vec::new();
        let studies = data["studies"].as_array().unwrap_or(&empty_vec);
        let total = data["totalCount"].as_u64().unwrap_or(studies.len() as u64);

        if studies.is_empty() {
            out.push_str(&format!("No studies found (total: {total}).\n"));
            return Ok(ToolOutput { content: out, metadata: None });
        };

        out.push_str(&format!("Total studies found: {total}\n\n"));

        for (i, study) in studies.iter().enumerate() {
            let protocol = &study["protocolSection"];
            let id = &protocol["identificationModule"];
            let status_mod = &protocol["statusModule"];
            let design = &protocol["designModule"];
            let conditions = &protocol["conditionsModule"];
            let contacts = &protocol["contactsLocationsModule"];

            let brief_title = id["briefTitle"].as_str().unwrap_or("(no title)");
            let nct_id = id["nctId"].as_str().unwrap_or("?");
            let overall_status = status_mod["overallStatus"].as_str().unwrap_or("UNKNOWN");
            let brief_summary = protocol["descriptionModule"]["briefSummary"].as_str().unwrap_or("");

            // Conditions
            let cond_list: String = conditions["conditions"].as_array()
                .map(|a| a.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join(", "))
                .unwrap_or_default();

            // Interventions
            let int_list: String = design["interventions"].as_array()
                .map(|a| a.iter().filter_map(|v| v["name"].as_str()).collect::<Vec<_>>().join(", "))
                .unwrap_or_default();

            // Phase
            let phases: String = design["phases"].as_array()
                .map(|a| a.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join(", "))
                .unwrap_or_default();

            // Sponsor
            let sponsor = id["organization"]["fullName"].as_str().unwrap_or("");

            // Enrollment
            let enrollment = design["enrollmentInfo"]["count"].as_u64().unwrap_or(0);

            // Locations
            let locs: String = contacts["locations"].as_array()
                .map(|a| a.iter().filter_map(|v| {
                    let facility = v["facility"].as_str().unwrap_or("");
                    let country = v["country"].as_str().unwrap_or("");
                    if facility.is_empty() && country.is_empty() { None }
                    else { Some(format!("{}, {}", facility, country).trim_matches(',').trim_matches(' ').to_string()) }
                }).collect::<Vec<_>>().join("; "))
                .unwrap_or_default();

            out.push_str(&format!("{}. **{brief_title}**\n", i + 1));
            out.push_str(&format!("   NCT: `{nct_id}` | Status: **{overall_status}**\n"));
            if !cond_list.is_empty() {
                out.push_str(&format!("   Conditions: {cond_list}\n"));
            }
            if !int_list.is_empty() {
                out.push_str(&format!("   Interventions: {int_list}\n"));
            }
            if !phases.is_empty() {
                out.push_str(&format!("   Phase: {phases}\n"));
            }
            if !sponsor.is_empty() {
                out.push_str(&format!("   Sponsor: {sponsor}\n"));
            }
            if enrollment > 0 {
                out.push_str(&format!("   Enrollment: {enrollment}\n"));
            }
            if !locs.is_empty() {
                let loc_preview: String = locs.chars().take(200).collect();
                out.push_str(&format!("   Locations: {loc_preview}\n"));
            }
            if !brief_summary.is_empty() {
                let preview: String = brief_summary.chars().take(250).collect();
                out.push_str(&format!("   Summary: {preview}...\n"));
            }
            out.push_str(&format!("   https://clinicaltrials.gov/study/{nct_id}\n"));
            out.push('\n');
        }

        Ok(ToolOutput { content: out, metadata: None })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clinical_trials_name_and_class() {
        let tool = ClinicalTrialsTool::new();
        assert_eq!(tool.name(), "clinical_trials_search");
        assert_eq!(tool.class(), ToolClass::ReadOnly);
    }

    #[test]
    fn test_clinical_trials_description_contains_keywords() {
        let tool = ClinicalTrialsTool::new();
        let desc = tool.description();
        assert!(desc.contains("ClinicalTrials.gov"), "Should mention ClinicalTrials.gov");
        assert!(desc.contains("NCT"), "Should mention NCT number");
    }

    #[test]
    fn test_clinical_trials_input_schema_has_required_fields() {
        let schema = ClinicalTrialsTool::new().input_schema();
        assert!(schema["required"].as_array().unwrap().iter().any(|v| v == "query"),
            "query should be required");
    }

    #[test]
    fn test_clinical_trials_schema_enums() {
        let schema = ClinicalTrialsTool::new().input_schema();
        // status enum
        if let Some(status) = schema["properties"]["status"]["enum"].as_array() {
            let values: Vec<&str> = status.iter().filter_map(|v| v.as_str()).collect();
            assert!(values.contains(&"recruiting"));
            assert!(values.contains(&"completed"));
            assert!(values.contains(&"terminated"));
        }
        // phase enum
        if let Some(phase) = schema["properties"]["phase"]["enum"].as_array() {
            let values: Vec<&str> = phase.iter().filter_map(|v| v.as_str()).collect();
            assert!(values.contains(&"phase_1"));
            assert!(values.contains(&"phase_3"));
        }
        // study_type enum
        if let Some(st) = schema["properties"]["study_type"]["enum"].as_array() {
            let values: Vec<&str> = st.iter().filter_map(|v| v.as_str()).collect();
            assert!(values.contains(&"interventional"));
            assert!(values.contains(&"observational"));
        }
    }

    #[test]
    fn test_clinical_trials_max_results_default() {
        let schema = ClinicalTrialsTool::new().input_schema();
        let props = schema["properties"].as_object().unwrap();
        assert!(props.contains_key("max_results"), "Should have max_results");
    }

    #[test]
    fn test_clinical_trials_no_query_error() {
        let tool = ClinicalTrialsTool::new();
        let input = json!({}); // missing query
        let ctx = ToolContext { working_dir: ".".into(), session_id: "test".into() };
        let cancel = CancellationToken::new();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(tool.execute(input, &ctx, cancel));
        assert!(result.is_err(), "Should error on missing query");
        if let Err(e) = result {
            let msg = format!("{e}");
            assert!(msg.contains("missing"), "Error should mention missing");
        }
    }
}
