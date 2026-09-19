package com.example.ops;

import org.springframework.scheduling.annotation.Scheduled;
import org.springframework.stereotype.Component;

@Component
public class Cleanup {
    @Scheduled(cron = "0 0 4 * * *")
    public void purge() {}
}
