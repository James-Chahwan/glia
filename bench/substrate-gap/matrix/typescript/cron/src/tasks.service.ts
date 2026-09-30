import { Injectable } from "@nestjs/common";
import { Cron } from "@nestjs/schedule";

@Injectable()
export class TasksService {
  @Cron("0 3 * * *")
  purge() {
    return 0;
  }
}
