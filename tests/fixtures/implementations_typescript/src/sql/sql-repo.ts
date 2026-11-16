import { BaseRepository } from "../adapters/base-repository";

export class SqlRepo extends BaseRepository {
  find() { return "sql"; }
}
