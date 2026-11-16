export interface OrderRepository {
  find(): string;
}

export class InMemoryRepository implements OrderRepository {
  find() { return "memory"; }
}
