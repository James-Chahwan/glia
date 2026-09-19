import { PrismaClient } from "@prisma/client";

const prisma = new PrismaClient();

export const listUsers = () => prisma.user.findMany();
