// No `vscode` import here: a look-alike API registers nothing with the host.
const workspace = {
	onDidRenameFiles(handler: () => void) {
		handler();
	},
};

export function wire() {
	workspace.onDidRenameFiles(() => {});
}
