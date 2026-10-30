# typed: true

module ActiveRecord
  class Rollback < StandardError; end

  class Base
    def self.transaction(&blk); end
  end
end
