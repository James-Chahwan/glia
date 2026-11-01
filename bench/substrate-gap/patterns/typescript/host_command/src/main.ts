// An Obsidian plugin (obsidianmd/obsidian-sample-plugin's shape): the default
// export extends Plugin and registers commands and events on `this`.
import { App, Modal, Notice, Plugin, PluginSettingTab, TAbstractFile } from 'obsidian';

export default class SamplePlugin extends Plugin {
	async onload() {
		this.addCommand({
			id: 'open-sample-modal',
			name: 'Open sample modal',
			callback: () => {
				new SampleModal(this.app).open();
			},
		});
		this.addCommand({
			id: 'toggle-sample',
			name: 'Toggle sample',
			checkCallback: this.toggleSample.bind(this),
		});
		this.registerEvent(this.app.vault.on('create', this.onCreate.bind(this)));
		this.registerDomEvent(document, 'click', (evt: MouseEvent) => {
			new Notice(`clicked ${evt.x}`);
		});
		this.addSettingTab(new SampleSettingTab(this.app, this));
	}

	toggleSample(checking: boolean): boolean {
		return !checking;
	}

	onCreate(file: TAbstractFile) {
		new Notice(file.path);
	}

	onunload() {}
}

class SampleModal extends Modal {
	onOpen() {
		this.contentEl.setText('Woah!');
	}

	onClose() {
		this.contentEl.empty();
	}
}

class SampleSettingTab extends PluginSettingTab {
	constructor(app: App, private plugin: SamplePlugin) {
		super(app, plugin);
	}

	display(): void {
		this.containerEl.empty();
	}
}
