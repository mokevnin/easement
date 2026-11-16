import { OrderRepository } from "../ports/order-repository";

export function cached(): OrderRepository {
  class Inner implements OrderRepository {
    find() { return "inner"; }
  }
  return new Inner();
}
