# typed: true

module Shop
  module Db
    class Client
      def self.connect
        new
      end

      def query(sql)
        sql
      end
    end
  end
end
