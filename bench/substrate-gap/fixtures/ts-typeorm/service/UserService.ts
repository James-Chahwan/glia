import { getRepository } from "typeorm";
import { User } from "../entity/User";

export async function listUsers() {
  return getRepository(User).find();
}
