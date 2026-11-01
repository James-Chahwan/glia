// A VS Code extension subscribes to the editor's events.
import * as vscode from 'vscode';
import { window } from 'vscode';

function onSave(doc: vscode.TextDocument) {
	console.log(`saved ${doc.fileName}`);
}

function onEditor(editor: vscode.TextEditor | undefined) {
	window.showInformationMessage(editor ? editor.document.fileName : 'none');
}

export function activate(context: vscode.ExtensionContext) {
	context.subscriptions.push(vscode.workspace.onDidSaveTextDocument(onSave));
	context.subscriptions.push(window.onDidChangeActiveTextEditor(onEditor));
}
