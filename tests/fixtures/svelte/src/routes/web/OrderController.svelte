<script lang="ts">
  import { onMount } from 'svelte';
  import Order from '#lib/domain/Order.svelte';
  import OrderRepository from '../../lib/infra/OrderRepository.svelte';
</script>

<OrderRepository />
<Order />
