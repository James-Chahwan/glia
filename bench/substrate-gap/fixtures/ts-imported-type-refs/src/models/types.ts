export type UserDto = {
  id: string;
  name: string;
};

export interface Out {
  ok: boolean;
}

export enum Role {
  Admin = "admin",
  Member = "member",
}

export function makeUser(): UserDto {
  return { id: "1", name: "a" };
}
