// VS Code calls an extension's exported activate / deactivate by name.
import * as vscode from 'vscode';

let channel: vscode.OutputChannel | undefined;

function openChannel(): vscode.OutputChannel {
	return vscode.window.createOutputChannel('Hello');
}

export function activate(context: vscode.ExtensionContext) {
	channel = openChannel();
	context.subscriptions.push(channel);
}

export function deactivate() {
	channel?.dispose();
}
