import { Entity, PrimaryGeneratedColumn, Column } from "typeorm";

@Entity("app_users")
export class User {
  @PrimaryGeneratedColumn() id!: number;
  @Column() email!: string;
}
