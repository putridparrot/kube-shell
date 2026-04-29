use std::time::Duration;

const DEFAULT_AI_URL: &str = "http://localhost:11434";
const DEFAULT_AI_MODEL: &str = "llama3.2";
const DEFAULT_ASK_PROMPT_TEMPLATE: &str = "You are a Kubernetes expert assistant. Current kubectl context: {context}. Current namespace: {namespace}. Answer the following question clearly and concisely. Use plain text without markdown formatting.\n\nQuestion:\n{question}";
const DEFAULT_EXPLAIN_PROMPT_TEMPLATE: &str = "You are a Kubernetes expert. Current kubectl context: {context}. Current namespace: {namespace}. Explain the following kubectl output clearly and concisely, highlighting anything notable. Use plain text without markdown formatting.\n\nCommand:\n{command}\n\nOutput:\n{output}";

pub struct AiClient {
    pub url: String,
    pub model: String,
    ask_prompt_template: String,
    explain_prompt_template: String,
    client: reqwest::blocking::Client,
}

impl AiClient {
    pub fn new(
        url: String,
        model: String,
        ask_prompt_template: String,
        explain_prompt_template: String,
    ) -> Self {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(120))
            .build()
            .unwrap_or_default();
        AiClient {
            url,
            model,
            ask_prompt_template,
            explain_prompt_template,
            client,
        }
    }

    pub fn default() -> Self {
        Self::new(
            DEFAULT_AI_URL.to_string(),
            DEFAULT_AI_MODEL.to_string(),
            DEFAULT_ASK_PROMPT_TEMPLATE.to_string(),
            DEFAULT_EXPLAIN_PROMPT_TEMPLATE.to_string(),
        )
    }

    fn render_prompt(template: &str, replacements: &[(&str, &str)]) -> String {
        let mut prompt = template.to_string();
        for (key, value) in replacements {
            prompt = prompt.replace(key, value);
        }
        prompt
    }

    fn generate(&self, prompt: &str) -> Result<String, String> {
        let url = format!("{}/api/generate", self.url.trim_end_matches('/'));
        let body = serde_json::json!({
            "model": self.model,
            "prompt": prompt,
            "stream": false
        });

        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .map_err(|e| format!("Cannot reach AI at {}: {e}", self.url))?;

        if !resp.status().is_success() {
            return Err(format!(
                "AI returned HTTP {} — check that the model '{}' is available",
                resp.status(),
                self.model
            ));
        }

        let json: serde_json::Value = resp
            .json()
            .map_err(|e| format!("Failed to parse AI response: {e}"))?;

        json["response"]
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| "AI response missing 'response' field".to_string())
    }

    /// Ask a free-form Kubernetes question.
    pub fn ask(&self, question: &str, context: &str, namespace: &str) -> Result<String, String> {
        let prompt = Self::render_prompt(
            &self.ask_prompt_template,
            &[
                ("{question}", question),
                ("{context}", context),
                ("{namespace}", namespace),
            ],
        );
        self.generate(&prompt)
    }

    /// Explain the output of a kubectl command.
    pub fn explain(
        &self,
        output: &str,
        command: Option<&str>,
        context: &str,
        namespace: &str,
    ) -> Result<String, String> {
        let command = command.unwrap_or("kubectl <unknown>");
        let prompt = Self::render_prompt(
            &self.explain_prompt_template,
            &[
                ("{output}", output),
                ("{command}", command),
                ("{context}", context),
                ("{namespace}", namespace),
            ],
        );
        self.generate(&prompt)
    }

    /// Print current configuration and test connectivity.
    pub fn status(&self) -> Result<(), String> {
        println!("AI configuration:");
        println!("  URL:   {}", self.url);
        println!("  Model: {}", self.model);

        let tags_url = format!("{}/api/tags", self.url.trim_end_matches('/'));
        match self.client.get(&tags_url).send() {
            Ok(resp) if resp.status().is_success() => {
                println!("  Status: connected");
            }
            Ok(resp) => {
                println!("  Status: reachable (HTTP {})", resp.status());
            }
            Err(e) => {
                println!("  Status: unreachable ({e})");
            }
        }

        Ok(())
    }
}
