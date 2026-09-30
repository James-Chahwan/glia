package com.example;

import org.jobrunr.jobs.annotations.Job;

public class EmailJobs {
    @Job(name = "send-welcome-email")
    public void sendWelcome(String userId) {
    }
}
