import { useState, useEffect } from "react";

interface TimerProps {
    intervalMs: number;
}

export function Timer({ intervalMs }: TimerProps) {
    const [tick, setTick] = useState(0);

    useEffect(() => {
        const id = setInterval(() => setTick(tick + 1), intervalMs);
        return () => clearInterval(id);
    }, [intervalMs]);

    return (
        <View>
            <Text>Tick: {tick}</Text>
        </View>
    );
}
