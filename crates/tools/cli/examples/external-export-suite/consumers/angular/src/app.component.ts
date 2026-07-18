import { Component } from "@angular/core";
// Generated standalone Angular wrappers (value props → @Input,
// callbacks → @Output EventEmitters).
import { GreeterComponent } from "external-export-suite-components/angular/greeter.component";
import { StepperComponent } from "external-export-suite-components/angular/stepper.component";

@Component({
  selector: "app-root",
  standalone: true,
  imports: [GreeterComponent, StepperComponent],
  template: `
    <main>
      <h1>Angular consumer</h1>

      <section>
        <h2>&lt;Greeter&gt;</h2>
        <idl-greeter-ng [name]="'World'" (greet)="greets = greets + 1"></idl-greeter-ng>
        <p>greet events: <strong>{{ greets }}</strong></p>
      </section>

      <section>
        <h2>&lt;Stepper&gt; (controlled)</h2>
        <idl-stepper-ng [label]="'Count'" [value]="value" (step)="value = $event"></idl-stepper-ng>
        <p>value: <strong>{{ value }}</strong></p>
      </section>
    </main>
  `,
})
export class AppComponent {
  greets = 0;
  value = 0; // host owns the stepper's value
}
