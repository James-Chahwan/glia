// An MCP server (the TypeScript SDK README's shape): a McpServer registers
// its tools, prompts and resources by name; the client calls them by name.
import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { z } from "zod";

const notes: string[] = [];

function save(text: string) {
	notes.push(text);
}

async function addNote({ text }: { text: string }) {
	save(text);
	return { content: [{ type: "text", text: "saved" }] };
}

async function findNotes({ query }: { query: string }) {
	const hits = notes.filter((n) => n.includes(query));
	return { content: [{ type: "text", text: hits.join("\n") }] };
}

function summarize() {
	return { messages: [{ role: "user", content: { type: "text", text: notes.join("\n") } }] };
}

async function allNotes(uri: URL) {
	return { contents: [{ uri: uri.href, text: notes.join("\n") }] };
}

const server = new McpServer({ name: "notes", version: "1.0.0" });

server.tool("add_note", { text: z.string() }, addNote);
server.registerTool("search", { description: "Search the notes" }, findNotes);
server.prompt("summarize", summarize);
server.registerResource("all_notes", "notes://all", {}, allNotes);

// A client calls a remote server's tool: it registers nothing here.
export async function remote() {
	const client = new Client({ name: "notes-client", version: "1.0.0" });
	client.tool("remote_add", addNote);
	return client.callTool({ name: "remote_add", arguments: {} });
}
