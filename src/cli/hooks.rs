use super::{build, flags, load};
use crate::error::Error;

pub(super) fn cmd_hooks(cfg_path: &str, args: &[String]) -> Result<i32, Error> {
    let (mut force, mut remove) = (None, None);
    let (path, fs) = flags("hooks", cfg_path, args, |fs| {
        force = Some(fs.bool(
            "force",
            "rewrite existing webhooks (after changing the secret)",
        ));
        remove = Some(fs.bool("remove", "delete the webhooks gitsync created"));
    })?;
    let force = force.is_some_and(|f| fs.on(f));
    let remove = remove.is_some_and(|f| fs.on(f));
    let a = build(load(&path)?)?;
    let Some(hooks) = a.src.hooks() else {
        return Err(Error::Config(format!(
            "{} has no webhook management here: create the webhook by hand, pointing at {}",
            a.cfg.source.kind,
            a.cfg.hook_url()
        )));
    };
    if remove {
        let mut n = 0;
        if hooks.remove_global(a.eng.system_hook()) {
            n += 1;
            a.eng.set_system_hook(0);
        }
        for r in a.eng.list_all()? {
            n += hooks.remove_repo(&r);
        }
        println!("removed {n} webhook(s)");
        return Ok(0);
    }
    a.eng.setup_hooks(force);
    if a.eng.hook_mode() == "repo" {
        for r in a.eng.list_all()? {
            if !a.eng.selected(&r).0 {
                continue;
            }
            let res = hooks.repo(&r, force).unwrap_or_else(|e| e.to_string());
            println!("{}: {res}", r.full());
        }
    }
    Ok(0)
}
