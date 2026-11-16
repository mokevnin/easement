import { OrderRepository } from "../ports/order-repository";

export class FakeRepository implements OrderRepository {
  find() { return "fake"; }
}
