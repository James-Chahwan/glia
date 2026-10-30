# Deposits go to the vault contract; this reader only opens local files.
def readme(archive):
    return archive.read("docs/readme.md")
