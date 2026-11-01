// The extension reads the settings its package.json contributes.
import * as vscode from 'vscode';

export function greeting(): string {
	const config = vscode.workspace.getConfiguration('helloWorld');
	return config.get<string>('greeting', 'Hello');
}

export function excited(): boolean {
	return vscode.workspace.getConfiguration('helloWorld').get('excited') === true;
}
