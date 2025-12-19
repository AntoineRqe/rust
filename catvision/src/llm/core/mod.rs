use std::{collections::HashMap, sync::Arc};
use std::error::Error;
use serde_json::Value;
use tokio::runtime::Runtime;

use crate::category::CATEGORIES;
use crate::ctx::Ctx;

use crate::llm::providers::gemini::generating::{async_gemini_fetch_chat_completion, GeminiResult, async_gemini_handle_cached_content};


enum LLMResult {
    Gemini(GeminiResult),
}

pub fn parse_llm_output(
    domains: &[String],
    content: &str,
) -> Result<HashMap<String, Vec<String>>, Box<dyn std::error::Error>> {
    // Avoid unnecessary trim/allocation
    let content = content.trim_start_matches(|c: char| c.is_whitespace())
                         .trim_end_matches(|c: char| c.is_whitespace());

    
    let json: Value = serde_json::from_str(content)?;
    let obj = json.as_object().ok_or("Expected top-level JSON object")?;

    // Pre allocate the result map for better performance
    let mut result: HashMap<String, Vec<String>> = HashMap::with_capacity(domains.len());

    for domain in domains {
        let value = obj.get(domain)
            .ok_or(format!("Domain '{}' not found in JSON", domain))?;

        let arr = value.as_array()
            .ok_or(format!("Expected array for domain '{}'", domain))?;

        if arr.is_empty() {
            return Err(format!("No categories found for domain '{}'", domain).into());
        }

        // Collect categories as &str to avoid allocations
        let categories: Vec<String> = arr.iter()
            .filter_map(|v| v.as_str())
            .map(|s| s.to_string())
            .collect();

        if categories.is_empty() {
            return Err(format!("No valid string categories for domain '{}'", domain).into());
        }

        result.insert(domain.to_string(), categories);
    }

    Ok(result)
}

fn write_domain_in_garbage_file(domains: &[String], id: usize) {
    use std::fs::OpenOptions;
    use std::io::Write;

    let garbage_file = format!("garbage_domains_{}.txt", id);

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&garbage_file)
        .unwrap_or_else(|_| panic!("Unable to open {}", garbage_file));

    for domain in domains {
        writeln!(file, "{}", domain)
            .unwrap_or_else(|_| panic!("Unable to write to {}", garbage_file));
        println!("Written garbage domain '{}' to {}", domain, garbage_file);
    }
}

async fn async_llm_classify_domains(domains: &[String], ctx: Arc<Ctx>, cache_name: Arc<Option<String>>, id: usize) -> Result<LLMResult, Box<dyn Error + Send + Sync>> {
    let mut gemini_result = GeminiResult {
        failed: 0,
        retried: 0,
        cost: 0.0,
        cache_saving: 0.0,
        categories: HashMap::new(),
    };

    let mut retries_chunk = 0;
    
    loop {
        if retries_chunk == 3 {
            eprintln!("Thread {} Failed to get LLM response after 3 attempts for chunk starting with domain: {}", id, domains[0]);
            gemini_result.failed += domains.len();
            write_domain_in_garbage_file(domains, id);
            return Err("Max retries reached for LLM request".into());
        }

        match async_gemini_fetch_chat_completion(domains, &ctx, (*cache_name).clone(), &mut gemini_result).await {
            Ok(()) => {
                println!("Thread {} Successfully processed chunk starting with domain: {}", id, domains[0]);
                break;
            },
            Err(e) => {
                eprintln!("Thread {} Error during LLM request (attempt {}): {}", id, retries_chunk + 1, e);
                retries_chunk += 1;
                gemini_result.retried += 1;
            }
        };
    }

    // println!("Thread {} Final Gemini Result: {}", id, gemini_result);

    Ok(LLMResult::Gemini(GeminiResult {
        retried: gemini_result.retried,
        failed: gemini_result.failed,
        cost: gemini_result.cost,
        cache_saving: gemini_result.cache_saving,
        categories: gemini_result.categories,
    }))
}

type DynError = Box<dyn std::error::Error + Send + Sync>;

async fn llm_runtime(domains: &[String], ctx: Arc<Ctx>) -> Result<GeminiResult, DynError> {

    let mut chunks = domains.chunks(ctx.config.chunk_size);
    let mut processed_domains = 0;
    let total_domains = domains.len();

    // Aggregate results
    let mut final_gemini_result = GeminiResult {
        failed: 0,
        retried: 0,
        cost: 0.0,
        cache_saving: 0.0,
        categories: HashMap::with_capacity(4000000),
    };

    while let Some(_) = chunks.clone().next() { // loop until no more chunks
        let ctx = Arc::clone(&ctx);
        let mut handles = Vec::with_capacity(ctx.as_ref().config.max_threads);

        let cache_name = match async_gemini_handle_cached_content(&ctx, &mut final_gemini_result.cost).await {
            Ok(cache_name) => Arc::new(cache_name),
            Err(e) => {
                eprintln!("Error handling cached content: {}", e);
                while let Some(chunk) = chunks.next() {
                    processed_domains += chunk.len();
                    println!("Skipping LLM runtime on {} with chunk size [{}-{}]/{} due to caching error",
                        ctx.config.model[0], processed_domains - chunk.len(), processed_domains, total_domains
                    );
                    final_gemini_result.failed += chunk.len();
                    write_domain_in_garbage_file(chunk, 666);
                }
                break;
            }
        };
    
        for id in 0..ctx.as_ref().config.max_threads {
            if let Some(chunk) = chunks.next() {
                let ctx = Arc::clone(&ctx);
                let domains_vec: Vec<String> = chunk.iter().map(|s| s.to_string()).collect();
                let cache_name = Arc::clone(&cache_name);

                processed_domains += domains_vec.len();
            
                tracing::info!(
                    "Starting LLM runtime on {} with chunk [{}-{}]/{}",
                    ctx.config.model[0],
                    processed_domains - domains_vec.len(),
                    processed_domains,
                    total_domains
                );

                let handle = tokio::spawn( {        
                    async move {
                        async_llm_classify_domains(&domains_vec, ctx, cache_name, id).await
                    }
                });
        
                handles.push(handle);
            } else {
                break;
            }
        }

        // Wait for all tasks to complete
        let futures_results: Vec<_> = futures::future::join_all(handles).await;

        for (i, result) in futures_results.into_iter().enumerate() {
            match result {
                Err(join_err) => {
                    eprintln!("Task {i} PANICKED: {join_err}");
                }
                Ok(task_result) => match task_result {
                    Ok(LLMResult::Gemini(gemini_result)) => {
                        final_gemini_result.failed += gemini_result.failed;
                        final_gemini_result.retried += gemini_result.retried;
                        final_gemini_result.cost += gemini_result.cost;
                        final_gemini_result.cache_saving += gemini_result.cache_saving;
                        final_gemini_result.categories.extend(gemini_result.categories);
                    },
                    Err(e) => {
                        eprintln!("Error in LLM classification task: {}", e);
                    }
                },
            }
        }

        tracing::info!(
            "Completed LLM runtime on {} with chunk size [{}-{}]/{}",
            ctx.config.model[0],
            processed_domains - ctx.config.chunk_size * ctx.config.max_threads,
            processed_domains,
            total_domains
        );

    }

    tracing::info!(
        "LLM runtime completed on {} for total domains: {}",
        ctx.config.model[0],
        total_domains
    );

    Ok(final_gemini_result)

}

pub fn sync_llm_runtime(domains: &[String], ctx: &Ctx) -> Result<GeminiResult, DynError> {
    // Create a new Tokio runtime
    let rt = Runtime::new().expect("Failed to create Tokio runtime");

    let ctx = std::sync::Arc::new(ctx.clone());

    // Block on the async function
    rt.block_on(llm_runtime(domains, ctx))
}

pub fn generate_full_prompt(domains: &[String], nb_propositions: usize) -> String {
    let domains_str = domains.join("; ");
    let prompt = format!("
        Tu es un expert en catégorisation de sites web et domaines internet.
Ta mission est d'analyser les domaines fournis et de les classer dans les catégories appropriées en suivant rigoureusement les règles ci-dessous.

**CONTRAINTE MAJEURE :** Puisque l'accès direct aux URL n'est pas possible, tu dois **SIMULER cet accès en utilisant l'outil de recherche web** pour obtenir les informations sur le contenu réel. Si une information essentielle n'est pas trouvable via la recherche, indique clairement les informations exactes que tu aurais cherché.

## Méthodologie obligatoire

Pour CHAQUE domaine :

1.  **Génération des Requêtes :** Effectue impérativement les recherches web suivantes, sans aucune modification du nom de domaine fourni :
    * **Requête Principale :** Utilise EXACTEMENT la requête \"Qu'est-ce que <domaine_entier>\" pour obtenir le contenu réel.
    * **Requête Secondaire (si sous-domaine) :** Si le domaine contient un sous-domaine, exécute une seconde recherche avec \"Qu'est-ce que <domaine_principal_plus_tld>\".
2.  **Source d'Analyse :** Base-toi EXCLUSIVEMENT sur le résultat des recherches web pour toute décision de catégorisation.
3.  **Application des Règles :** Applique les règles de catégorisation dans l'ordre de priorité en fonction du contenu réel trouvé.
4.  **Limitation de Catégories :** Attribue au maximum **{nb_propositions}** catégories distinctes, classées par ordre de pertinence décroissante.
5.  **Pertinence :** Si une catégorie te semble très pertinente, ne cherche pas à en ajouter d'autres moins pertinentes.

## Règles de Catégorisation par Catégorie (Priorité et Descriptions)

**Ordre de Priorité des Règles**

* Flux financiers institutionnels → Banques
* Spécialisation thématique > Généraliste (ex: blog auto → Voiture, pas Blog)
* Produits d'une seule catégorie → Intérêts/Loisirs (sauf si catégorie dédiée existe)
* Service principal > Infrastructure technique
* Restaurants/Fast-food → TOUJOURS Voyage/Tourisme/Sortie

### Armes / Explosifs

Sites de vente, information ou promotion d'armes à feu, munitions, explosifs
Armureries en ligne ou physiques
Clubs de tir, vente d'équipement
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
Exemples : reddit.com, skyrock.com

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
IMPORTANT : Si la majorité des produits sont dans UNE seule catégorie alors privilégie la catégorisation en te basant sur le type de produits vendus (exemple vêtements -> \"Intérêts / Loisirs\")
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
IMPORTANT : Privilégie la catégorisation du domaine principal si ce dernier est identifiable plutôt que d'utiliser le sous-domaine technique
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
Préfère cette catégorie plutôt que E-Commerce / Enchères pour les sites de ventes ayant un accent thématique
Sports, hobbies, collections, bricolage
Blogs/médias spécialisés dans un loisir spécifique
Météo, jardinage, photographie, musique (outils/équipement)
Exemples : decathlon.com, thomann.de, marcopolo-expert.fr, modelisme.com

### Jeux d'argent

Paris sportifs, casinos en ligne, poker
Loteries, jeux d'argent
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
Sondages rémunérés
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

DOMAINES À CLASSER :
{domains_str}

CATÉGORIES DISPONIBLES :
{CATEGORIES}

## FORMAT DE RÉPONSE

Classifie chaque nom de domaine dans un unique dictionnaire JSON plat.
La sortie doit être exactement un bloc délimité par \"```json\" et \"```\" et contenant uniquement ce dictionnaire : chaque domaine en clé, une seule catégorie en valeur.
Aucune variante de catégories : uniquement les libellés prédéfinis.
Aucun texte dans le bloc.
Le dictionnaire doit contenir tous les domaines donnés dans le prompt d'entrée, vérifie bien ce point, N'OUBLIE PAS DE DOMAINES.

Exemple:
```json
{{\"www.renault.fr\": [\"Voitures / Mécaniques\"], \"twitter.com\": [\"Réseaux sociaux\"]}}
```

Interdictions absolues :

PAS de Markdown
PAS de texte explicatif
PAS de commentaires
PAS de paragraphes supplémentaires
PAS de citations ou mise en forme
");
    prompt
}

pub fn generate_prompt_with_cached_content(domains: &[String]) -> String {
    let domains_str = domains.join("; ");

let prompt = format!(

"
N'utilise aucune réponse que tu as donnée auparavant.
Utilise seulement les instructions contenues dans cached_content.

Voici les domaines à classer :
{domains_str}

");

    prompt
}

pub fn generate_cached_prompt(nb_propositions: usize) -> String {

let prompt = format!(

"
Tu es un expert en catégorisation de sites web et domaines internet.
Ta mission est d'analyser les domaines fournis et de les classer dans les catégories appropriées en suivant rigoureusement les règles ci-dessous.

Tu as accès au contenu réel des sites web via un outil de recherche web GoogleSearch. Utilise cet outil pour obtenir les informations nécessaires à la catégorisation.

## Méthodologie obligatoire

Pour CHAQUE domaine :

1.  **Génération des Requêtes :** Effectue impérativement les recherches web suivantes, sans aucune modification du nom de domaine fourni :
    * **Requête Principale :** Utilise EXACTEMENT la requête \"Qu'est-ce que <domaine_entier>\" pour obtenir le contenu réel.
    * **Requête Secondaire (si sous-domaine) :** Si le domaine contient un sous-domaine, exécute une seconde recherche avec \"Qu'est-ce que <domaine_principal_plus_tld>\".
2.  **Source d'Analyse :** Base-toi EXCLUSIVEMENT sur le résultat des recherches web pour toute décision de catégorisation.
3.  **Application des Règles :** Applique les règles de catégorisation dans l'ordre de priorité en fonction du contenu réel trouvé.
4.  **Limitation de Catégories :** Attribue au maximum **{nb_propositions}** catégories distinctes, classées par ordre de pertinence décroissante.
5.  **Pertinence :** Si une catégorie te semble très pertinente, ne cherche pas à en ajouter d'autres moins pertinentes.

## Règles de Catégorisation par Catégorie (Priorité et Descriptions)

**Ordre de Priorité des Règles**

* Flux financiers institutionnels → Banques
* Spécialisation thématique > Généraliste (ex: blog auto → Voiture, pas Blog)
* Produits d'une seule catégorie → Intérêts/Loisirs (sauf si catégorie dédiée existe)
* Service principal > Infrastructure technique
* Restaurants/Fast-food → TOUJOURS Voyage/Tourisme/Sortie

### Armes / Explosifs

Sites de vente, information ou promotion d'armes à feu, munitions, explosifs
Armureries en ligne ou physiques
Clubs de tir, vente d'équipement
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
Exemples : reddit.com, skyrock.com

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
IMPORTANT : Si la majorité des produits sont dans UNE seule catégorie alors privilégie la catégorisation en te basant sur le type de produits vendus (exemple vêtements -> \"Intérêts / Loisirs\")
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
IMPORTANT : Privilégie la catégorisation du domaine principal si ce dernier est identifiable plutôt que d'utiliser le sous-domaine technique
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
Préfère cette catégorie plutôt que E-Commerce / Enchères pour les sites de ventes ayant un accent thématique
Sports, hobbies, collections, bricolage
Blogs/médias spécialisés dans un loisir spécifique
Météo, jardinage, photographie, musique (outils/équipement)
Exemples : decathlon.com, thomann.de, marcopolo-expert.fr, modelisme.com

### Jeux d'argent

Paris sportifs, casinos en ligne, poker
Loteries, jeux d'argent
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
Sondages rémunérés
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

CATÉGORIES DISPONIBLES :
{CATEGORIES}

## FORMAT DE RÉPONSE

Classifie chaque nom de domaine dans un unique dictionnaire JSON plat.
La sortie doit être exactement un bloc délimité par \"```json\" et \"```\" et contenant uniquement ce dictionnaire : chaque domaine en clé, une seule catégorie en valeur.
Aucune variante de catégories : uniquement les libellés prédéfinis.
Aucun texte dans le bloc.
Le dictionnaire doit contenir tous les domaines donnés dans le prompt d'entrée, vérifie bien ce point, N'OUBLIE PAS DE DOMAINES.

Exemple:
```json
{{\"www.renault.fr\": [\"Voitures / Mécaniques\"], \"twitter.com\": [\"Réseaux sociaux\"]}}
```

Interdictions absolues :

PAS de Markdown
PAS de texte explicatif
PAS de commentaires
PAS de paragraphes supplémentaires
PAS de citations ou mise en forme
");

    prompt
}