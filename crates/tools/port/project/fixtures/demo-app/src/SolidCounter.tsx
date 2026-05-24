import { createSignal, createEffect } from "solid-js";

interface CounterProps {
    initial?: number;
}

export function Counter(props: CounterProps) {
    const [count, setCount] = createSignal(props.initial ?? 0);

    createEffect(() => {
        console.log("count:", count());
    });

    return (
        <View>
            <Text>Count: {count()}</Text>
            <Button label="Inc" onClick={() => setCount(count() + 1)} />
        </View>
    );
}
