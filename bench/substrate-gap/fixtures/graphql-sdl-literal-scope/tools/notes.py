# How the resolver scan works: a line like
#   type Query {
# opens a root type block. Nothing in this file is a schema.
def scan(text):
    return text.splitlines()
