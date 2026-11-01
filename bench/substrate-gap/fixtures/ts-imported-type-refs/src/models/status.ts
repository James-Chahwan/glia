export const Status = {
  Active: "active",
  Disabled: "disabled",
} as const;

export type Status = (typeof Status)[keyof typeof Status];
