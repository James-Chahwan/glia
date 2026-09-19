from apscheduler.schedulers.blocking import BlockingScheduler
from apscheduler.triggers.cron import CronTrigger

from tasks import rollup

sched = BlockingScheduler()


@sched.scheduled_job("cron", hour=3, minute=15)
def nightly():
    pass


sched.add_job(rollup, CronTrigger.from_crontab("0 6 * * mon"))
