<script lang="ts">
    export let initial = 0;
    let count = initial;

    $: console.log('count:', count);

    function increment() {
        count = count + 1;
    }
</script>

<View>
    <Text>Count: {count}</Text>
    <Button label="Inc" on:click={increment} />
</View>
