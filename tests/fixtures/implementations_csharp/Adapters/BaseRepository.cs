using Acme.Ports;

namespace Acme.Adapters
{
    public abstract class BaseRepository : IOrderRepository
    {
        public abstract string Find();
    }

    public class SqlServerRepository : IOrderRepository
    {
        public string Find() { return "sqlserver"; }
    }
}
