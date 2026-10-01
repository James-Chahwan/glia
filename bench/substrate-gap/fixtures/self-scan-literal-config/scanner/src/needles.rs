//! A scanner's own fixtures: `os.getenv("JWT_SECRET")`, LaunchDarkly's
//! `ldClient.variation("beta-search", ctx, false)`, node-cron's
//! `cron.schedule("*/5 * * * *", tick)` and `new MongoClient(url)`.

pub fn mode() -> String {
    std::env::var("APP_MODE").unwrap_or_default()
}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_the_literals() {
        let env = "import os\nsecret = os.getenv('JWT_SECRET')\n";
        let flag = "import * as ld from 'launchdarkly-node-server-sdk';\nconst on = ldClient.variation('beta-search', ctx, false);";
        let cron = "const cron = require('node-cron');\ncron.schedule('*/5 * * * *', tick);";
        let beat = "CELERY_BEAT_SCHEDULE = {'cleanup': {'task': 'tasks.cleanup', 'schedule': crontab(minute=0)}}";
        let db = "const db = new MongoClient(url);";
        assert!(!env.is_empty() && !flag.is_empty() && !cron.is_empty() && !beat.is_empty() && !db.is_empty());
    }
}
