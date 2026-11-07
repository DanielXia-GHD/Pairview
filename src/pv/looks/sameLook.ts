// Pairview: a recipe keeps a look's full path, which differs once the app moves (a dev build to a
// packaged one, another Mac). The backend finds such a look by its folder and file name
// (`pv::looks::current_path`), so "is this the same look?" has to ask the same question. The folder
// counts: `pv_film_luts` and `film_luts` hold different LUTs under the same file names.
const tail = (path: string) => path.split(/[\\/]/).slice(-2).join('/');

export function isSameLook(a: string | null | undefined, b: string | null | undefined): boolean {
  if (!a || !b) return false;
  return a === b || tail(a) === tail(b);
}
