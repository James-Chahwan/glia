from pkg import helper
from blinker import signal
sent = signal('sent')

@sent.connect
def on_sent(sender):
    helper()

def run():
    sent.send('x')
