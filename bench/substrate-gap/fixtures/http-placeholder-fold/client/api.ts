export const getUser = (id: number) => fetch(`/users/${id}`);
export const getFile = (p: string) => fetch(`/files/${p}`);
