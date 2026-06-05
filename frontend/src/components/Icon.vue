<template>
  <svg class="icon" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="1.8" aria-hidden="true">
    <path v-if="name === 'sun'" stroke-linecap="round" stroke-linejoin="round" d="M12 4V2M12 22v-2M4 12H2M22 12h-2M5 5l-1.4-1.4M20.4 20.4 19 19M19 5l1.4-1.4M3.6 20.4 5 19M12 16a4 4 0 1 0 0-8 4 4 0 0 0 0 8Z" />
    <path v-else stroke-linecap="round" stroke-linejoin="round" d="M20 15.5A8.5 8.5 0 0 1 8.5 4 8.5 8.5 0 1 0 20 15.5Z" />
  </svg>
</template>

<script setup lang="ts">
defineProps<{
  name: 'sun' | 'moon'
}>()
</script>
