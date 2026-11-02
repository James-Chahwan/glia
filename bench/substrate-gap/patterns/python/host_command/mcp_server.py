# An MCP server registers its tools, prompts and resources with its host
# (the MCP SDK's FastMCP, the quickstart's shape): a client calls a tool or
# prompt by name and reads a resource by its URI.
from mcp.server.fastmcp import FastMCP

mcp = FastMCP("notes")

NOTES = []


def save(text):
    NOTES.append(text)
    return len(NOTES)


@mcp.tool()
def add_note(text: str) -> int:
    return save(text)


@mcp.tool(name="search")
def find_notes(query: str) -> list:
    return [n for n in NOTES if query in n]


@mcp.prompt()
def summarize(topic: str) -> str:
    return f"Summarize the notes about {topic}."


@mcp.resource("notes://all")
def all_notes() -> str:
    return "\n".join(NOTES)


if __name__ == "__main__":
    mcp.run()
