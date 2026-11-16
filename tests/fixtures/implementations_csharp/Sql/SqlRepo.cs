using Acme.Adapters;

namespace Acme.Sql
{
    public class SqlRepo : BaseRepository
    {
        public override string Find() { return "sql"; }
    }
}
