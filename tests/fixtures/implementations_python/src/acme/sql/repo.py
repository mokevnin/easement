from acme.adapters.base import BaseRepository


class SqlRepo(BaseRepository):
    def find(self) -> str:
        return "sql"
