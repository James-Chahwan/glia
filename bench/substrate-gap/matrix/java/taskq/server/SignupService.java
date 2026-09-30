package com.example;

import org.jobrunr.scheduling.BackgroundJob;

public class SignupService {
    private final EmailJobs emailJobs = new EmailJobs();

    public void signup(String userId) {
        BackgroundJob.enqueue(() -> emailJobs.sendWelcome(userId));
    }
}
