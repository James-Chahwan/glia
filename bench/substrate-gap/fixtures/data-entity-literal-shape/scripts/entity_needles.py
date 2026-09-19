"""Scanner needle table: the strings are data, not models or tables."""

NEEDLES = ["mongoose.model(", "TableName:", "dynamodb.Table("]


def find_needle(text):
    for n in NEEDLES:
        at = text.find(n)
        if at >= 0:
            return at
    return None
