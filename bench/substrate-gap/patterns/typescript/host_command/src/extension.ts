// A VS Code extension registers the commands its package.json declares.
import * as vscode from 'vscode';

function sayHello() {
	vscode.window.showInformationMessage('Hello World!');
}

export function activate(context: vscode.ExtensionContext) {
	context.subscriptions.push(vscode.commands.registerCommand('extension.helloWorld', sayHello));
	context.subscriptions.push(
		vscode.commands.registerCommand('extension.reload', () => {
			sayHello();
		}),
	);
}
