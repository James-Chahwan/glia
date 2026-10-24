//! A release tool: it launches the deployer, and its test holds a CronJob
//! manifest as text. Neither the manifest text nor the commented-out launch
//! is a schedule or a CLI invocation.

pub fn launch() {
    std::process::Command::new("deployer").arg("up").spawn().ok();
}

// Command::new("ghost")

#[cfg(test)]
const K8S: &str = "apiVersion: batch/v1
kind: CronJob
metadata:
  name: weekly-report
spec:
  schedule: '0 6 * * 1'
  jobTemplate:
    spec:
      template:
        spec:
          containers:
          - name: report
            command: [report]
";

#[cfg(test)]
mod tests {
    #[test]
    fn manifest_names_a_cronjob() {
        assert!(super::K8S.contains("CronJob"));
    }
}
