import os

# Read-side only: RUNTIME_MODE is defined nowhere in this fixture, so its
# CONFIG_KEY node exists in exactly one graph and must carry NO env cell.
RUNTIME_MODE = os.environ["RUNTIME_MODE"]
