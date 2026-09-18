// Mentions graphql-request in prose only; no GraphQL here.
pub fn send_request(agent: &ureq::Agent, url: &str) -> String {
    agent.request("GET", url).call().unwrap().into_string().unwrap()
}
