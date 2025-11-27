use std::{collections::{HashMap}};
use std::error::Error;
use tokio::runtime::Runtime;

use crate::gemini::{GeminiResult, async_gemini_fetch_chat_completion};
use crate::category::{CATEGORIES, SUB_CATEGORIES_JSON};
use crate::ctx::Ctx;

enum LLMResult {
    Gemini(GeminiResult),
}

pub fn parse_llm_output(domains: &[String], content: &str, nb_propositions: usize) -> Result<HashMap<String, Vec<String>>, Box<dyn std::error::Error>> {
    let content = content.trim().to_string();

    let mut llm_categories: HashMap<String, Vec<String>> = HashMap::new();
    let mut consumed_domains = domains.len() as i32;

    for (index, domain_line) in content.lines().enumerate() {
        let mut parts = domain_line.split(';');
        let domain = parts.next().unwrap_or("").trim().to_string();

        if !domains.contains(&domain) {
            println!("Warning: domain mismatch. Expected: {}, Got: {}", domains[index], domain);
            return Err("Domain mismatch in response".into());
        }

        let mut tmp_propositions = Vec::new();

        for _ in 0..nb_propositions {
            if let Some(category) = parts.next() {
                let cat_trimmed = category.trim().to_string();
                tmp_propositions.push(cat_trimmed.clone());
            } else {
                tmp_propositions.push("".to_string());
            }
        }

        llm_categories.insert(domain, tmp_propositions);

        consumed_domains -= 1;
    }

    match consumed_domains {
        0 => Ok(llm_categories),
        _ => {
            println!("Warning: Not all domains were processed. Remaining: {}", consumed_domains);
            Err("Not all domains processed".into())
        }
    }
}


async fn async_llm_classify_domains(domains: &[String], ctx: &Ctx) -> Result<LLMResult, Box<dyn Error>> {
    let mut gemini_result = GeminiResult {
        failed: 0,
        retried: 0,
        cost: 0.0,
        categories: HashMap::new(),
    };

    for domain_chunk in domains.chunks(ctx.config.chunk_size) {
        let mut retries_chunk = 0;
        loop {
            if retries_chunk == 3 {
                eprintln!("Failed to get LLM response after 3 attempts for chunk starting with domain: {}", domain_chunk[0]);
                gemini_result.failed += 1;
                break;
            }
    
            match async_gemini_fetch_chat_completion(domain_chunk, &ctx.config.model[0], ctx.config.max_domain_propositions, gemini_result.clone()).await {
                Ok(result) => {
                    gemini_result = result;
                    break;
                },
                Err(e) => {
                    eprintln!("Error during LLM request (attempt {}): {}", retries_chunk + 1, e);
                    retries_chunk += 1;
                }
            };
        }
    }

    println!("Final Gemini Result: {}", gemini_result);
    Ok(LLMResult::Gemini(GeminiResult {
        retried: gemini_result.retried,
        failed: gemini_result.failed,
        cost: gemini_result.cost,
        categories: gemini_result.categories,
    }))
}

async fn llm_runtime(domains: &[String], ctx: &Ctx) -> Result<HashMap<String, Vec<String>>, Box<dyn std::error::Error>>{
    let mut handles = vec![];
    println!("Starting {} LLM runtime on {}...", if ctx.config.support_multithread { "with multithreading" } else { "without multithreading" }, ctx.config.model[0]);

    match async_llm_classify_domains(domains, ctx).await {
        Ok(LLMResult::Gemini(gemini_result)) => {
            handles.push(tokio::spawn(async move {
                gemini_result
            }));
        },
        Err(e) => {
            eprintln!("Error during LLM classification: {}", e);
        }
    }

    // Wait for all tasks to complete
    let futures_results: Vec<_> = futures::future::join_all(handles)
    .await
    .into_iter()
    .collect::<Result<Vec<_>, _>>()
    .unwrap();

    println!("All tasks completed!");

    Ok(futures_results.into_iter().fold(HashMap::new(), |mut acc, res| {
        for (domain, categories) in res.categories {
            acc.insert(domain, categories);
        }
        acc
    }))
}

pub fn sync_llm_runtime(domains: &[String], ctx: &Ctx) -> Result<HashMap<String, Vec<String>>, Box<dyn std::error::Error>> {
    // Create a new Tokio runtime
    let rt = Runtime::new().expect("Failed to create Tokio runtime");

    // Block on the async function
    rt.block_on(llm_runtime(domains, ctx))
}

pub fn generate_prompt(domains: &[String], nb_propositions: usize) -> String {
    let domains_str = domains.join("; ");

let prompt = format!(
"Tu es un expert en catégorisation de sites web et domaines internet. Ta mission est d'analyser des domaines et de les classer dans les catégories appropriées en suivant strictement les règles ci-dessous.

## Méthodologie obligatoire

Pour CHAQUE domaine :

* Effectue une recherche web pour comprendre son contenu réel et son usage principal
* Analyse le nom du domaine, le service principal, et le contexte d'utilisation
* Applique les règles de catégorisation dans l'ordre de priorité
* Attribue au maximum {nb_propositions} catégories distinctes, classé par ordre de pertinence décroissante.
* Si une catégorie te semble très pertinente, ne cherche pas à en ajouter d'autres moins pertinentes.

## Règles de Catégorisation par Catégorie

### Armes / Explosifs

Sites de vente, information ou promotion d'armes à feu, munitions, explosifs
Armureries en ligne ou physiques
Clubs de tir avec vente d'équipement
Exemples : armurerie-lyon.fr, gunbroker.com, defense-shooting.com

### Banques / Services financiers / Investissement

RÈGLE PRIORITAIRE : Tout flux financier institutionnel (hors achats en ligne et petites annonces)
Banques traditionnelles et néobanques
Organismes de transfert d'argent
Services d'aide sociale avec versements (CAF, sécurité sociale)
Trading, investissement, crypto-monnaies
Exemples : bnpparibas.fr, paypal.com, binance.com, caf.fr, ameli.fr

### Blogs / Forums

EXCEPTION : Si le blog/forum est thématique spécialisé, privilégier la catégorie thématique
Forums de discussion généralistes
Blogs personnels sans thématique dominante
Plateformes communautaires multi-thèmes
Exemples : reddit.com, jeuxvideo.com (partie forum), skyrock.com

### Chat / Communication

Messageries instantanées (texte, voix, vidéo)
Applications de communication en temps réel
VoIP et téléphonie internet
Note : Sous-domaines techniques dédiés à la communication (ex: teams.microsoft.com)
Exemples : signal.org, discord.com, zoom.us, slack.com

### Contenus pirates

Streaming illégal, téléchargement non autorisé
Sites de déblocage, contournement de DRM
Partage illégal de contenus protégés
Exemples : yts.mx, rarbg.to, sci-hub.se, soap2day.to

### Drogue alcool et tabac

Vente de produits liés au cannabis (légal ou non)
Sites de vente d'alcool, cigares, cigarettes
Boutiques de chicha, accessoires de fumeurs
Exemples : lacentrale-dubidou.com, vinatis.com, cigarworld.de, headshop.fr

### E-Commerce / Enchères

RÈGLE : Plateformes vendant des produits VARIÉS de différentes catégories
Marketplaces généralistes
Sites d'enchères multi-produits, site de commissaire priseurs
ATTENTION : Si tous les produits sont dans UNE seule catégorie → \"Intérêts / Loisirs\"
Exemples : amazon.com, ebay.com, cdiscount.com, vinted.com

### Email

Services de messagerie électronique
Webmails dédiés
Exemples : gmail.com, outlook.com, protonmail.com, mail.yahoo.com

### Emploi

Sites de recherche d'emploi
Recrutement, offres de postes
Plateformes de mise en relation professionnelle pour l'emploi
Exemples : indeed.fr, pole-emploi.fr, monster.com, apec.fr
Contre-exemples : linkedin.com → Réseaux sociaux car on peut aller sur ce site sans forcément chercher un emploi

### Enseignement

Établissements scolaires, universités
Plateformes d'apprentissage en ligne
Cours, formations, certifications
Exemples : coursera.org, openclassrooms.com, univ-paris.fr, skillshare.com

### Domaine technique

RÈGLE : CDN, DNS, APIs, infrastructure GÉNÉRIQUE utilisable par tous
EXCEPTION : Si le domaine technique sert UN SEUL service identifiable → catégorie du service
Services cloud génériques, hébergement technique
Exemples : cloudflare.com, akamai.net, amazonaws.com (si générique)
Contre-exemples : tiktokcdn.com → Réseaux sociaux

### Fraude scolaire

Services de rédaction de devoirs/thèses
Vente de diplômes frauduleux
Tricherie académique
Exemples : essayshark.com, diploma-mill.com

### Gouvernement / Administration

Sites officiels d'État, ministères
Services publics administratifs (hors aide sociale financière)
Exemples : service-public.fr, interieur.gouv.fr, ants.gouv.fr

### Hebergement web / FAI

Fournisseurs d'accès internet
Hébergeurs de sites web
Registrars de domaines
Exemples : ovh.com, orange.fr (partie FAI), gandi.net

### Hébergement de fichiers

Stockage cloud personnel
Partage de fichiers
IMPORTANT : Se baser sur le service principal, pas les sous-domaines techniques
Exemples : dropbox.com, wetransfer.com, mega.nz, icloud.com (partie stockage)

### Intelligence artificielle

Chatbots IA, générateurs de contenu
Outils d'IA pour création/analyse
Plateformes d'IA conversationnelle ou générative
Exemples : openai.com, midjourney.com, perplexity.ai, anthropic.com

### Intérêts / Loisirs

RÈGLE MAJEURE : Sites spécialisés vendant des produits d'UNE SEULE catégorie thématique
Sports, hobbies, collections, bricolage
Blogs/médias spécialisés dans un loisir spécifique
Météo, jardinage, photographie, musique (outils/équipement)
Exemples : decathlon.com, thomann.de, marcopolo-expert.fr, modelisme.com

### Jeux d'argent

Paris sportifs, casinos en ligne, poker
Loteries, jeux d'argent réglementés
Exemples : betclic.fr, pokerstars.com, fdj.fr, unibet.fr

### Moteur de recherche

Moteurs de recherche web généralistes
Exemples : google.com (page d'accueil), bing.com, duckduckgo.com, ecosia.org

### Téléchargement de fichiers

Stores d'applications mobiles/logicielles
Plateformes de distribution de logiciels
Exemples : play.google.com, apps.apple.com, softonic.com, ninite.com

### Streaming / Télévision / Radio

Plateformes vidéo légales (SVOD, replay)
Streaming audio, podcasts, webradios
Chaînes TV en ligne
Exemples : netflix.com, spotify.com, twitch.tv, radiofrance.fr

### Médias / Actualités

EXCEPTION : Médias spécialisés → catégorie thématique (ex: sport.bbc.com → Intérêts/Loisirs)
Presse généraliste, sites d'information
Agences de presse
Exemples : lemonde.fr, bfmtv.com, theguardian.com, afp.com

### Itinéraires / Cartographie

GPS, cartes, calcul d'itinéraires
Géolocalisation, navigation
Suivi temps rééel, traçage
Exemples : waze.com, openstreetmap.org, viamichelin.fr, here.com, treinposities.nl

### Occulte / Secte

Voyance, astrologie, ésotérisme
Organisations sectaires
Pratiques occultes
Exemples : evozen.fr, astrocenter.fr, medium-marabout.com

### Petites annonces

Plateformes de vente entre particuliers
Annonces classées multi-catégories
Exemples : kijiji.ca, craigslist.org, marktplaats.nl, avito.ma

### Politique / Droit / Social

Partis politiques, campagnes électorales
Cabinets d'avocats, sites juridiques
Mouvements sociaux, syndicats
Prisons, justice (hors administration)
Exemples : vie-publique.fr, avocats.fr, amnesty.org, cgt.fr

### Pornographie / Nudité / Images à caractère sexuel

Contenus explicites pour adultes
Sites de webcam adultes, escorts
Exemples : pornhub.com, onlyfans.com, xvideos.com

### Prise de contrôle à distance

Logiciels de bureau à distance
Accès distant, support technique
Exemples : anydesk.com, teamviewer.com, parsec.app

### Publicité

Régies publicitaires
Plateformes d'affichage de publicités
Exemples : doubleclick.net, taboola.com, adroll.com

### Religion

Sites religieux, lieux de culte
Vente de produit dédiés à la religion, articles liturgiques
Organisations confessionnelles
Exemples : vatican.va, mosquee-lyon.org, torah.org

### Réseaux sociaux

Plateformes de partage social
RÈGLE : Sous-domaines techniques dédiés à UN réseau social → Réseaux sociaux
Microblogging, réseaux visuels
Exemples : instagram.com, tiktok.com, x.com, mastodon.social

### Santé

Hôpitaux, cliniques, professionnels de santé
Pharmacies en ligne, vente de médicaments
Information médicale
EXCEPTION : Forums santé → Blogs/Forums
Exemples : doctolib.fr, 1mg.com, mayoclinic.org, ameli.fr (partie info)

### Sites / Applications de rencontre

Applications et sites de rencontre sentimentale
Matchmaking, dating
Exemples : tinder.com, meetic.fr, bumble.com, happn.com

### Services / Sites d'entreprises

Sites corporate, B2B
Services professionnels génériques
Entreprises technologiques (hors produits spécifiques)
Exemples : microsoft.com (corporate), salesforce.com, adobe.com

### Traduction

Services de traduction en ligne
Dictionnaires multilingues
Exemples : deepl.com, reverso.net, linguee.com

### Téléphonie mobile

Opérateurs mobiles
Services liés aux smartphones (hors apps)
Exemples : bouyguestelecom.fr, verizon.com, t-mobile.com

VPNs / Filtres / Proxies / Redirection

### Réseaux privés virtuels
Proxies, anonymisation
Contrôle parental technique
Exemples : expressvpn.com, protonvpn.com, tor.org

### Virus / Piratage informatique

Malwares, hacking, exploits
Sites de distribution de virus
Tutoriels de piratage malveillant
Exemples : exploit-db.com (si malveillant), sites de RATs

### Voitures / Mécaniques

RÈGLE : Sites SPÉCIALISÉS automobile/mécanique
Constructeurs, concessionnaires
Pièces détachées, mécanique
Blogs/médias automobiles
Exemples : renault.fr, oscaro.com, caradisiac.com, garage-moderne.fr

### Voyage / Tourisme / Sortie

RÈGLE IMPORTANTE : TOUS les restaurants et fast-foods
Réservation de voyages, hôtels
Activités touristiques, loisirs sortants
Guides de voyage, attractions
Exemples : booking.com, tripadvisor.com, mcdonalds.fr, parc-asterix.fr

### Autres

Uniquement si aucune catégorie ne correspond après analyse approfondie
À utiliser en dernier recours

Ordre de Priorité des Règles

Recherche web obligatoire pour chaque domaine
Flux financiers institutionnels → Banques
Spécialisation thématique > Généraliste (ex: blog auto → Voiture, pas Blog)
Produits d'une seule catégorie → Intérêts/Loisirs (sauf si catégorie dédiée existe)
Sous-domaines techniques → Privilégier l'usage du service parent si identifiable
Service principal > Infrastructure technique
Restaurants/Fast-food → TOUJOURS Voyage/Tourisme/Sortie

CATÉGORIES DISPONIBLES :
{CATEGORIES}

Pour aider la classification, voici une liste des sous catégories pour déterminer la catégorie principale à choisir en format JSON :
{SUB_CATEGORIES_JSON}

DOMAINES À CLASSER :
{domains_str}

Tu DOIS utiliser les outils suivants :

url_context – permet de récupérer le contenu d’URLs
google_search – permet d’effectuer une recherche pour identifier des catégories

Seul résultat accepté :

Une ligne CSV par domaine, sous la forme exacte
domaine;cat1;cat2;cat3

Interdictions absolues :

PAS de Markdown
PAS de texte explicatif
PAS de commentaires
PAS de paragraphes supplémentaires
PAS de citations ou mise en forme
");

    prompt
}

pub fn _generate_cached_prompt(nb_propositions: usize) -> String {

let prompt = format!(
"Tu es un expert en catégorisation de sites web et domaines internet. Ta mission est d'analyser des domaines et de les classer dans les catégories appropriées en suivant strictement les règles ci-dessous.

## Méthodologie obligatoire

Pour CHAQUE domaine :

* Effectue une recherche web pour comprendre son contenu réel et son usage principal
* Analyse le nom du domaine, le service principal, et le contexte d'utilisation
* Applique les règles de catégorisation dans l'ordre de priorité
* Attribue au maximum {nb_propositions} catégories distinctes, classé par ordre de pertinence décroissante.
* Si une catégorie te semble très pertinente, ne cherche pas à en ajouter d'autres moins pertinentes.

## Règles de Catégorisation par Catégorie

### Armes / Explosifs

Sites de vente, information ou promotion d'armes à feu, munitions, explosifs
Armureries en ligne ou physiques
Clubs de tir avec vente d'équipement
Exemples : armurerie-lyon.fr, gunbroker.com, defense-shooting.com

### Banques / Services financiers / Investissement

RÈGLE PRIORITAIRE : Tout flux financier institutionnel (hors achats en ligne et petites annonces)
Banques traditionnelles et néobanques
Organismes de transfert d'argent
Services d'aide sociale avec versements (CAF, sécurité sociale)
Trading, investissement, crypto-monnaies
Exemples : bnpparibas.fr, paypal.com, binance.com, caf.fr, ameli.fr

### Blogs / Forums

EXCEPTION : Si le blog/forum est thématique spécialisé, privilégier la catégorie thématique
Forums de discussion généralistes
Blogs personnels sans thématique dominante
Plateformes communautaires multi-thèmes
Exemples : reddit.com, jeuxvideo.com (partie forum), skyrock.com

### Chat / Communication

Messageries instantanées (texte, voix, vidéo)
Applications de communication en temps réel
VoIP et téléphonie internet
Note : Sous-domaines techniques dédiés à la communication (ex: teams.microsoft.com)
Exemples : signal.org, discord.com, zoom.us, slack.com

### Contenus pirates

Streaming illégal, téléchargement non autorisé
Sites de déblocage, contournement de DRM
Partage illégal de contenus protégés
Exemples : yts.mx, rarbg.to, sci-hub.se, soap2day.to

### Drogue alcool et tabac

Vente de produits liés au cannabis (légal ou non)
Sites de vente d'alcool, cigares, cigarettes
Boutiques de chicha, accessoires de fumeurs
Exemples : lacentrale-dubidou.com, vinatis.com, cigarworld.de, headshop.fr

### E-Commerce / Enchères

RÈGLE : Plateformes vendant des produits VARIÉS de différentes catégories
Marketplaces généralistes
Sites d'enchères multi-produits, site de commissaire priseurs
ATTENTION : Si tous les produits sont dans UNE seule catégorie → \"Intérêts / Loisirs\"
Exemples : amazon.com, ebay.com, cdiscount.com, vinted.com

### Email

Services de messagerie électronique
Webmails dédiés
Exemples : gmail.com, outlook.com, protonmail.com, mail.yahoo.com

### Emploi

Sites de recherche d'emploi
Recrutement, offres de postes
Plateformes de mise en relation professionnelle pour l'emploi
Exemples : indeed.fr, pole-emploi.fr, monster.com, apec.fr
Contre-exemples : linkedin.com → Réseaux sociaux car on peut aller sur ce site sans forcément chercher un emploi

### Enseignement

Établissements scolaires, universités
Plateformes d'apprentissage en ligne
Cours, formations, certifications
Exemples : coursera.org, openclassrooms.com, univ-paris.fr, skillshare.com

### Domaine technique

RÈGLE : CDN, DNS, APIs, infrastructure GÉNÉRIQUE utilisable par tous
EXCEPTION : Si le domaine technique sert UN SEUL service identifiable → catégorie du service
Services cloud génériques, hébergement technique
Exemples : cloudflare.com, akamai.net, amazonaws.com (si générique)
Contre-exemples : tiktokcdn.com → Réseaux sociaux

### Fraude scolaire

Services de rédaction de devoirs/thèses
Vente de diplômes frauduleux
Tricherie académique
Exemples : essayshark.com, diploma-mill.com

### Gouvernement / Administration

Sites officiels d'État, ministères
Services publics administratifs (hors aide sociale financière)
Exemples : service-public.fr, interieur.gouv.fr, ants.gouv.fr

### Hebergement web / FAI

Fournisseurs d'accès internet
Hébergeurs de sites web
Registrars de domaines
Exemples : ovh.com, orange.fr (partie FAI), gandi.net

### Hébergement de fichiers

Stockage cloud personnel
Partage de fichiers
IMPORTANT : Se baser sur le service principal, pas les sous-domaines techniques
Exemples : dropbox.com, wetransfer.com, mega.nz, icloud.com (partie stockage)

### Intelligence artificielle

Chatbots IA, générateurs de contenu
Outils d'IA pour création/analyse
Plateformes d'IA conversationnelle ou générative
Exemples : openai.com, midjourney.com, perplexity.ai, anthropic.com

### Intérêts / Loisirs

RÈGLE MAJEURE : Sites spécialisés vendant des produits d'UNE SEULE catégorie thématique
Sports, hobbies, collections, bricolage
Blogs/médias spécialisés dans un loisir spécifique
Météo, jardinage, photographie, musique (outils/équipement)
Exemples : decathlon.com, thomann.de, marcopolo-expert.fr, modelisme.com

### Jeux d'argent

Paris sportifs, casinos en ligne, poker
Loteries, jeux d'argent réglementés
Exemples : betclic.fr, pokerstars.com, fdj.fr, unibet.fr

### Moteur de recherche

Moteurs de recherche web généralistes
Exemples : google.com (page d'accueil), bing.com, duckduckgo.com, ecosia.org

### Téléchargement de fichiers

Stores d'applications mobiles/logicielles
Plateformes de distribution de logiciels
Exemples : play.google.com, apps.apple.com, softonic.com, ninite.com

### Streaming / Télévision / Radio

Plateformes vidéo légales (SVOD, replay)
Streaming audio, podcasts, webradios
Chaînes TV en ligne
Exemples : netflix.com, spotify.com, twitch.tv, radiofrance.fr

### Médias / Actualités

EXCEPTION : Médias spécialisés → catégorie thématique (ex: sport.bbc.com → Intérêts/Loisirs)
Presse généraliste, sites d'information
Agences de presse
Exemples : lemonde.fr, bfmtv.com, theguardian.com, afp.com

### Itinéraires / Cartographie

GPS, cartes, calcul d'itinéraires
Géolocalisation, navigation
Suivi temps rééel, traçage
Exemples : waze.com, openstreetmap.org, viamichelin.fr, here.com, treinposities.nl

### Occulte / Secte

Voyance, astrologie, ésotérisme
Organisations sectaires
Pratiques occultes
Exemples : evozen.fr, astrocenter.fr, medium-marabout.com

### Petites annonces

Plateformes de vente entre particuliers
Annonces classées multi-catégories
Exemples : kijiji.ca, craigslist.org, marktplaats.nl, avito.ma

### Politique / Droit / Social

Partis politiques, campagnes électorales
Cabinets d'avocats, sites juridiques
Mouvements sociaux, syndicats
Prisons, justice (hors administration)
Exemples : vie-publique.fr, avocats.fr, amnesty.org, cgt.fr

### Pornographie / Nudité / Images à caractère sexuel

Contenus explicites pour adultes
Sites de webcam adultes, escorts
Exemples : pornhub.com, onlyfans.com, xvideos.com

### Prise de contrôle à distance

Logiciels de bureau à distance
Accès distant, support technique
Exemples : anydesk.com, teamviewer.com, parsec.app

### Publicité

Régies publicitaires
Plateformes d'affichage de publicités
Exemples : doubleclick.net, taboola.com, adroll.com

### Religion

Sites religieux, lieux de culte
Vente de produit dédiés à la religion, articles liturgiques
Organisations confessionnelles
Exemples : vatican.va, mosquee-lyon.org, torah.org

### Réseaux sociaux

Plateformes de partage social
RÈGLE : Sous-domaines techniques dédiés à UN réseau social → Réseaux sociaux
Microblogging, réseaux visuels
Exemples : instagram.com, tiktok.com, x.com, mastodon.social

### Santé

Hôpitaux, cliniques, professionnels de santé
Pharmacies en ligne, vente de médicaments
Information médicale
EXCEPTION : Forums santé → Blogs/Forums
Exemples : doctolib.fr, 1mg.com, mayoclinic.org, ameli.fr (partie info)

### Sites / Applications de rencontre

Applications et sites de rencontre sentimentale
Matchmaking, dating
Exemples : tinder.com, meetic.fr, bumble.com, happn.com

### Services / Sites d'entreprises

Sites corporate, B2B
Services professionnels génériques
Entreprises technologiques (hors produits spécifiques)
Exemples : microsoft.com (corporate), salesforce.com, adobe.com

### Traduction

Services de traduction en ligne
Dictionnaires multilingues
Exemples : deepl.com, reverso.net, linguee.com

### Téléphonie mobile

Opérateurs mobiles
Services liés aux smartphones (hors apps)
Exemples : bouyguestelecom.fr, verizon.com, t-mobile.com

VPNs / Filtres / Proxies / Redirection

### Réseaux privés virtuels
Proxies, anonymisation
Contrôle parental technique
Exemples : expressvpn.com, protonvpn.com, tor.org

### Virus / Piratage informatique

Malwares, hacking, exploits
Sites de distribution de virus
Tutoriels de piratage malveillant
Exemples : exploit-db.com (si malveillant), sites de RATs

### Voitures / Mécaniques

RÈGLE : Sites SPÉCIALISÉS automobile/mécanique
Constructeurs, concessionnaires
Pièces détachées, mécanique
Blogs/médias automobiles
Exemples : renault.fr, oscaro.com, caradisiac.com, garage-moderne.fr

### Voyage / Tourisme / Sortie

RÈGLE IMPORTANTE : TOUS les restaurants et fast-foods
Réservation de voyages, hôtels
Activités touristiques, loisirs sortants
Guides de voyage, attractions
Exemples : booking.com, tripadvisor.com, mcdonalds.fr, parc-asterix.fr

### Autres

Uniquement si aucune catégorie ne correspond après analyse approfondie
À utiliser en dernier recours

Ordre de Priorité des Règles

Recherche web obligatoire pour chaque domaine
Flux financiers institutionnels → Banques
Spécialisation thématique > Généraliste (ex: blog auto → Voiture, pas Blog)
Produits d'une seule catégorie → Intérêts/Loisirs (sauf si catégorie dédiée existe)
Sous-domaines techniques → Privilégier l'usage du service parent si identifiable
Service principal > Infrastructure technique
Restaurants/Fast-food → TOUJOURS Voyage/Tourisme/Sortie

CATÉGORIES DISPONIBLES :
{CATEGORIES}

Pour aider la classification, voici une liste des sous catégories pour déterminer la catégorie principale à choisir en format JSON :
{SUB_CATEGORIES_JSON}

Tu DOIS utiliser les outils suivants :

url_context – permet de récupérer le contenu d’URLs
google_search – permet d’effectuer une recherche pour identifier des catégories

Seul résultat accepté :

Une ligne CSV par domaine, sous la forme exacte
domaine;cat1;cat2;cat3

Interdictions absolues :

PAS de Markdown
PAS de texte explicatif
PAS de commentaires
PAS de paragraphes supplémentaires
PAS de citations ou mise en forme
");

    prompt
}