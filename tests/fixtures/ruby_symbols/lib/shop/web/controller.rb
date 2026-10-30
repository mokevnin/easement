# typed: true

require_relative "../db/client"
require_relative "../db/scoped"

module Shop
  module Web
    class Controller
      extend T::Sig

      sig { params(client: Shop::Db::Client).returns(T.untyped) }
      def show(client)
        client.query("x")
      end

      def index
        Shop::Db::Scoped.new.all
        Shop::Db::Client.connect
        raise ActiveRecord::Rollback
      end
    end
  end
end
