<script setup lang="ts">
import { ref, watchEffect, withDefaults } from 'vue';

interface CounterProps {
    initial?: number;
}

const props = withDefaults(defineProps<CounterProps>(), { initial: 0 });

const count = ref(props.initial);

watchEffect(() => {
    console.log('count:', count.value);
});

function increment() {
    count.value++;
}
</script>

<template>
    <View>
        <Text>Count: {{ count }}</Text>
        <Button label="Inc" @click="increment" />
    </View>
</template>

<style scoped>
.counter { padding: 12px; }
</style>
