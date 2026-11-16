import { OrderRepository } from "../ports/order-repository";

export class CachedRepo implements OrderRepository {
  find() { return "cached"; }
}
