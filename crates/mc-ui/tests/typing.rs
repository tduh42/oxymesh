//! How many presses typing takes (`docs/spec/ui.md` §7): a fixed set of French and English
//! messages, typed by a planner that takes, at each step, the insertion that adds the most
//! text per press (as someone who knows the keyboard would). A regression gate, and the
//! measure behind every change to the prediction or the layout.

use mc_ui::keyboard::{Cand, Key, Keyboard, Mode, Move, Stop};

const SENTENCES: [&str; 14] = [
    "Where are you now?",
    "On my way, see you at the refuge",
    "Can you call me back tonight?",
    "The road is closed after the bridge",
    "I will be there in ten minutes",
    "Thanks for the coffee!",
    "Battery low, talk later",
    "Bonjour, tu es là ?",
    "J'arrive dans dix minutes",
    "On se retrouve au refuge vers 16h",
    "La route est fermée après le pont",
    "Merci pour le café !",
    "Il neige beaucoup ici",
    "Je rentre ce soir",
];

/// The same sentences with the one-ring keyboard (before U11): 2,864 presses.
const BEFORE: usize = 2_864;

fn apply(k: &mut Keyboard, moves: &[Move]) -> Key {
    let mut key = Key::Typing;
    for m in moves {
        key = match m {
            Move::Next => {
                k.next(1);
                Key::Typing
            }
            Move::Prev => {
                k.prev();
                Key::Typing
            }
            Move::Open => k.insert(),
            Move::Back => k.delete(),
        };
    }
    key
}

/// Every candidate the keyboard offers right now.
fn offered(k: &Keyboard) -> Vec<Cand> {
    let mut out: Vec<Cand> = Vec::new();
    for s in k.stops() {
        let items: Vec<Cand> = match s {
            Stop::One(c) => vec![c.clone()],
            Stop::Row(items) => items.clone(),
        };
        for c in items {
            if !out.contains(&c) {
                out.push(c);
            }
        }
    }
    out
}

/// Presses to type `target` and send it; `log` gets the steps.
fn presses(target: &str, recent: Vec<String>, log: &mut Vec<String>) -> usize {
    let mut k = Keyboard::new(Mode::Message { limit: 160 }, "", recent);
    let mut total = 0;
    for _ in 0..400 {
        let mut best: Option<(usize, usize, Vec<Move>, Cand)> = None;
        for c in offered(&k) {
            let Some(route) = k.route(&c) else { continue };
            let mut t = k.clone();
            let gain = match apply(&mut t, &route) {
                Key::Done(s) if s == target => usize::MAX,
                Key::Done(_) | Key::Cancel => continue,
                Key::Typing => {
                    let ok = target.starts_with(t.text.as_str()) || target == t.text.trim_end();
                    let n = t.text.chars().count();
                    if !ok || n <= k.text.chars().count() {
                        continue;
                    }
                    n - k.text.chars().count()
                }
            };
            let cost = route.len();
            // Most text per press; for equal ratios, the cheaper.
            let better = best.as_ref().is_none_or(|(g, c0, _, _)| {
                gain == usize::MAX
                    || (*g != usize::MAX
                        && (gain * c0 > g * cost || (gain * c0 == g * cost && cost < *c0)))
            });
            if better {
                best = Some((gain, cost, route, c));
            }
        }
        let Some((gain, _, route, c)) = best else {
            panic!("stuck at {:?} for {target:?}", k.text);
        };
        total += route.len();
        log.push(format!("{:>2} {:?}", route.len(), c.label(k.mode)));
        if let Key::Done(_) = apply(&mut k, &route) {
            assert_eq!(gain, usize::MAX);
            return total;
        }
    }
    panic!("{target:?} never finished");
}

#[test]
fn typing_takes_at_least_40_percent_fewer_presses() {
    let mut sum = 0;
    for s in SENTENCES {
        let mut log = Vec::new();
        let p = presses(s, Vec::new(), &mut log);
        sum += p;
        println!(
            "{p:4} {s}  ({:.1}/char)",
            p as f32 / s.chars().count() as f32
        );
        if std::env::var("TYPING_LOG").is_ok() {
            println!("     {}", log.join(" | "));
        }
    }
    println!("{sum:4} total, {BEFORE} before ({}%)", sum * 100 / BEFORE);
    assert!(
        sum * 100 <= BEFORE * 60,
        "{sum} presses, more than 60% of {BEFORE}"
    );
}

/// What you wrote before is offered again: a word from your sent messages is completed.
#[test]
fn your_own_words_are_cheaper_the_second_time() {
    let recent =
        mc_ui::keyboard::recent_words([b"meet at the Aiguille hut".as_slice()].into_iter());
    let mut log = Vec::new();
    let with = presses("Aiguille", recent, &mut log);
    let without = presses("Aiguille", Vec::new(), &mut Vec::new());
    assert!(with < without / 2, "{with} vs {without}: {log:?}");
}

/// Every character of both languages can be typed, `où` included.
#[test]
fn french_characters_are_all_offered() {
    for s in [
        "Où",
        "Le garçon",
        "Un œuf",
        "Une île",
        "Noël",
        "Ça va",
        "En août",
        "Le maïs",
        "Déjà vu",
        "Ô",
        "Œuvre",
        "Bientôt sûr",
    ] {
        presses(s, Vec::new(), &mut Vec::new());
    }
}
