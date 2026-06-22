interface SegmentedOption<V extends string> {
  value: V;
  label: string;
}

interface SegmentedControlProps<V extends string> {
  label: string;
  value: V;
  options: Array<SegmentedOption<V>>;
  onChange: (value: V) => void;
}

export function SegmentedControl<V extends string>({
  label,
  value,
  options,
  onChange,
}: SegmentedControlProps<V>) {
  return (
    <div className="segmented-control" role="group" aria-label={label}>
      {options.map((option) => (
        <button
          key={option.value}
          type="button"
          className={`segmented-control__option${option.value === value ? " is-active" : ""}`}
          aria-pressed={option.value === value}
          onClick={() => onChange(option.value)}
        >
          {option.label}
        </button>
      ))}
    </div>
  );
}
