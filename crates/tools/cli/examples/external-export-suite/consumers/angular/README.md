# Angular consumer

Angular builds with its own toolchain (Angular CLI), so this folder holds
the idiomatic **source** rather than a pre-wired build. The two files —
[`src/app.component.ts`](src/app.component.ts) and
[`src/main.ts`](src/main.ts) — drop straight into any Angular app.

## Wire it into an Angular project

```bash
ng new my-app --standalone --skip-tests
cd my-app
npm install ../../path/to/dist/external   # the components package (file: dep)
# replace src/app.component.ts + src/main.ts with the ones here
ng serve
```

The generated wrappers are **standalone components**, so you just add them
to a component's `imports` and bind normally:

```html
<idl-greeter-ng [name]="'World'" (greet)="onGreet()"></idl-greeter-ng>
<idl-stepper-ng [label]="'Count'" [value]="value" (step)="value = $event"></idl-stepper-ng>
```

- Value props (`name`, `label`, `value`) are `@Input`s.
- Callbacks (`greet`, `step`) are `@Output` `EventEmitter`s; the carried
  value arrives as `$event`.

The wrappers already declare `CUSTOM_ELEMENTS_SCHEMA`, so no extra schema
wiring is needed in your own modules.
