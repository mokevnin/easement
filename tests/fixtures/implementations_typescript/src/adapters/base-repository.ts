import { OrderRepository } from "../ports/order-repository";

export abstract class BaseRepository implements OrderRepository {
  abstract find(): string;
}

export class PgRepository implements OrderRepository {
  find() { return "pg"; }
}
