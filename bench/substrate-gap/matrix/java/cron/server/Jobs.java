package com.example.jobs;

import static org.quartz.CronScheduleBuilder.cronSchedule;

import org.quartz.*;

public class Jobs {
    public void schedule(Scheduler s) throws SchedulerException {
        JobDetail job = JobBuilder.newJob(ReportJob.class).withIdentity("report").build();
        Trigger t = TriggerBuilder.newTrigger().withSchedule(cronSchedule("0 0/15 * * * ?")).build();
        s.scheduleJob(job, t);
    }
}
