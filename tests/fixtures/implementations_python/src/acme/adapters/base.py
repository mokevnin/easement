from acme.ports.repository import OrderRepository


class BaseRepository(OrderRepository):
    pass


class PgRepository(OrderRepository):
    def find(self) -> str:
        return "pg"
