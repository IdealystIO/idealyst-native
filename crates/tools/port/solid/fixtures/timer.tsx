import { createSignal, createEffect, onCleanup } from "solid-js";

interface TimerProps {
    intervalMs: number;
}

export function Timer(props: TimerProps) {
    const [tick, setTick] = createSignal(0);

    createEffect(() => {
        const id = setInterval(() => setTick(tick() + 1), props.intervalMs);
        onCleanup(() => clearInterval(id));
    });

    return (
        <View>
            <Text>Tick: {tick()}</Text>
        </View>
    );
}
