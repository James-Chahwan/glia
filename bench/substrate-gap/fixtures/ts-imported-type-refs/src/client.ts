import axios from "axios";
import type { AxiosResponse } from "axios";
import { UserDto, Out, makeUser } from "./models/types";
import { Role as AccessRole } from "./models";
import { Status } from "./models/status";
import Repo from "./repo";

export async function getUser(): Promise<UserDto> {
  const res = await axios.get<UserDto>("/api/user");
  return res.data;
}

type LocalDto = { v: number };

export function getLocal(): LocalDto {
  return { v: 1 };
}

export abstract class Tool {
  abstract run(input: UserDto): Out;
  abstract local(input: LocalDto): void;
}

export function grant(role: AccessRole): boolean {
  return role === AccessRole.Admin;
}

export function lookup(repo: Repo, id: string): string {
  return repo.find(id);
}

export function wrap<UserDto>(value: UserDto): UserDto {
  return value;
}

export function isActive(s: Status): boolean {
  return s === Status.Active;
}

export function raw(res: AxiosResponse<UserDto>): UserDto {
  return res.data;
}

export function useMaker() {
  return [makeUser];
}
