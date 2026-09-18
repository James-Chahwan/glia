class Notifier:
    def publish(self, message):
        """Write the message to the audit log."""
        print(message)


def publish(report):
    return report.render()


def run(notifier, report):
    notifier.publish(publish(report))
