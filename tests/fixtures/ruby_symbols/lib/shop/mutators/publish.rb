# typed: true

module Shop
  module Mutators
    class Publish
      def call
        ActiveRecord::Base.transaction do
          raise ActiveRecord::Rollback
        end
      end
    end
  end
end
