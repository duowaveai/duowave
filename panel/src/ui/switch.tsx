/** Переключатель «Стекла»: роль switch, подпись — для чтения с экрана. */
export function Switch({ on, label, disabled = false, onChange }: { on: boolean; label: string; disabled?: boolean; onChange: (on: boolean) => void }) {
    return <button type="button" class="switch" role="switch" aria-checked={on} aria-label={label} disabled={disabled} onClick={() => onChange(!on)} />;
}
