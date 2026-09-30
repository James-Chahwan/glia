using Hangfire;

public class EmailJobs
{
    [Queue("emails")]
    public void SendWelcome(string userId) { }
}
