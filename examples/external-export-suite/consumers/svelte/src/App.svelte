<script>
  // Generated Svelte wrappers, resolved from the components package.
  import Greeter from "external-export-suite-components/svelte/Greeter.svelte";
  import Stepper from "external-export-suite-components/svelte/Stepper.svelte";

  let greets = 0;
  let value = 0; // host owns the stepper's value
</script>

<main>
  <h1>Svelte consumer</h1>

  <section>
    <h2>&lt;Greeter&gt;</h2>
    <Greeter name="World" onGreet={() => (greets += 1)} />
    <p>greet events: <strong>{greets}</strong></p>
  </section>

  <section>
    <h2>&lt;Stepper&gt; (controlled)</h2>
    <Stepper label="Count" {value} onStep={(next) => (value = next)} />
    <p>value: <strong>{value}</strong></p>
  </section>
</main>

<style>
  main { font-family: system-ui, sans-serif; margin: 2rem; max-width: 40rem; }
</style>
