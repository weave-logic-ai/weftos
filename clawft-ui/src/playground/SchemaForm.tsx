import type { Field, FormValues } from "./core.ts";

const input =
  "w-full rounded-md border border-gray-600 bg-gray-800 px-2 py-1.5 text-sm text-gray-100 placeholder-gray-500 focus:border-blue-500 focus:outline-none";

interface Props {
  fields: Field[];
  values: FormValues;
  errors: Record<string, string>;
  onChange: (name: string, value: string | boolean) => void;
  idPrefix: string;
}

/** A form generated from the fields of a JSON Schema. */
export function SchemaForm({ fields, values, errors, onChange, idPrefix }: Props) {
  if (fields.length === 0) {
    return <p className="text-sm text-gray-400">This tool takes no arguments.</p>;
  }
  return (
    <div className="space-y-3">
      {fields.map((f) => {
        const id = `${idPrefix}-${f.name}`;
        const v = values[f.name];
        return (
          <div key={f.name}>
            <label htmlFor={id} className="mb-1 flex items-baseline gap-2 text-sm font-medium text-gray-200">
              <span className="font-mono">{f.name}</span>
              {f.required ? <span className="text-xs text-red-400">required</span> : null}
              <span className="text-xs font-normal text-gray-500">
                {f.kind === "json" ? `JSON ${f.typeHint}` : f.kind}
              </span>
            </label>
            {f.kind === "boolean" ? (
              <input
                id={id}
                type="checkbox"
                checked={v === true}
                onChange={(e) => onChange(f.name, e.target.checked)}
              />
            ) : f.kind === "enum" ? (
              <select
                id={id}
                className={input}
                value={typeof v === "string" ? v : ""}
                onChange={(e) => onChange(f.name, e.target.value)}
              >
                <option value="">(unset)</option>
                {f.enumValues?.map((o) => (
                  <option key={o} value={o}>
                    {o}
                  </option>
                ))}
              </select>
            ) : f.kind === "json" ? (
              <textarea
                id={id}
                rows={3}
                spellCheck={false}
                className={`${input} font-mono`}
                value={typeof v === "string" ? v : ""}
                onChange={(e) => onChange(f.name, e.target.value)}
                placeholder={f.typeHint === "array" ? "[]" : "{}"}
              />
            ) : (
              <input
                id={id}
                type={f.kind === "string" ? "text" : "number"}
                step={f.kind === "integer" ? 1 : "any"}
                className={input}
                value={typeof v === "string" ? v : ""}
                onChange={(e) => onChange(f.name, e.target.value)}
              />
            )}
            {f.description ? <p className="mt-1 text-xs text-gray-500">{f.description}</p> : null}
            {errors[f.name] ? <p className="mt-1 text-xs text-red-400">{errors[f.name]}</p> : null}
          </div>
        );
      })}
    </div>
  );
}
