import { useState, useEffect } from "react";

interface CounterProps {
    initial?: number;
}

export function Counter({ initial = 0 }: CounterProps) {
    const [count, setCount] = useState(initial);

    useEffect(() => {
        console.log("count:", count);
    }, [count]);

    return (
        <View>
            <Text>Count: {count}</Text>
            <Button label="Inc" onClick={() => setCount(count + 1)} />
        </View>
    );
}
