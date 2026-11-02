export interface Tab {
  title: string;
  focused: boolean;
}

export function focusTab(tab: Tab): Tab {
  return { ...tab, focused: true };
}
