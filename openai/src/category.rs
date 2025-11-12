pub const CATEGORIES: &[&str] = &[
    "Armes / Explosifs",
    "Autres",
    "Banques / Services financiers / Investissement",
    "Blogs / Forums",
    "Chat / Communication",
    "Contenus pirates",
    "Discours violent / incitation à la haine",
    "Drogue alcool et tabac",
    "E-Commerce",
    "Email",
    "Emploi",
    "Enseignement",
    "Domaine technique",
    "Fraude Scolaire",
    "Gouvernement / Administration",
    "Hébergement web, FAI",
    "Hébergement de fichiers",
    "Immigration",
    "Immobilier",
    "Intelligence Artificielle",
    "Intérêts / Loisirs",
    "Jeux d'argent",
    "Moteur de recherche",
    "Téléchargement de Fichiers",
    "Streaming, Télévision, Radio",
    "Médias, Actualités",
    "Navigation / Transport",
    "Occulte / Secte",
    "Petites annonces",
    "Politique / Droit / Social",
    "Pornographie / Nudité / Images à caractère sexuel",
    "Prise de contrôle à distance",
    "Publicité",
    "Religion",
    "Réseaux sociaux",
    "Santé",
    "Sites / Applications de rencontre",
    "Services / Sites d'entreprises",
    "Traduction",
    "Téléphonie Mobile",
    "VPNs, Filtres, Proxies et Redirection",
    "Virus / Piratage informatique",
    "Voitures, Mécaniques",
    "Voyage / tourisme / Sortie",
];


pub const SUB_CATEGORIES_JSON : &str = r#"
{
  "Armes / Explosifs": [
    "Armes, Chasse, Equipement de Sécurité",
    "Sites décrivant des moyens de créer du matériel dangereux (explosif, poison)"
  ],
  "Autres": [
    "IP Non Classée",
    "Priorité Temporaire",
    "Site Indisponible",
    "Site à Accès Restreint",
    "URL Non Classée",
    "UT1 - Reaffected"
  ],
  "Banques / Services financiers / Investissement": [
    "Banques, Assurances, Caisses",
    "Investissement, Bourse, Placement",
    "UT1 - Financial"
  ],
  "Blogs / Forums": [
    "Blog",
    "Forum, Wiki",
    "Forum, Wiki Professionnels",
    "Pages Personnelles",
    "UT1 - Blog",
    "UT1 - Forums"
  ],
  "Chat / Communication": [
    "Chat",
    "Envoi de Textos et MMS",
    "Téléphonie par Internet, VoIP"
  ],
  "Contenus pirates": [
    "Contrefaçon",
    "Musiques, Films, Logiciels Piratés",
    "Peer to Peer",
    "Sites distribuant des logiciels ou des vidéos pirates"
  ],
  "Discours violent / incitation à la haine": [
    "Atteinte Physique et Morale",
    "Contenu Agressif, de Mauvais Goût",
    "Racisme, Discrimination, Révisionnisme",
    "Sites racistes, antisémites, incitant à la haine",
    "Terrorisme, Incitation à la Violence, Explosifs et Poisons"
  ],
  "Drogue alcool et tabac": [
    "Alcool et Tabac",
    "Alcool et Tabac Condamnés par la Loi Française",
    "Promotion et Vente de Drogue",
    "UT1 - Drogue",
    "Vente de Médicaments Condamnée par la Loi Française"
  ],
  "E-Commerce": [
    "Commerce en Ligne",
    "Enchères en Ligne",
    "UT1 - Shopping"
  ],
  "Email": [
    "UT1 - Webmail",
    "Webmail"
  ],
  "Emploi": [
    "Emploi, Recrutement"
  ],
  "Enseignement": [
    "Enseignement"
  ],
  "Domaine technique": [
    "Caches",
    "Domaines Parkés",
    "Réducteurs d'URL",
    "Serveurs de Statistiques",
    "Sites pour désinfecter et mettre à jour des ordinateurs",
    "CDN et Non Définissable"
  ],
  "Fraude Scolaire": [
    "Sites qui expliquent comment tricher aux examens"
  ],
  "Gouvernement / Administration": [
    "Administrations"
  ],
  "Hebergement web, FAI": [
    "Hébergement de Sites, FAI"
  ],
  "Hébergement de fichiers": [
    "Hébergement de fichiers",
    "Stockage de Données en Ligne"
  ],
  "Immigration": [
    "Immigration Clandestine et Travail Illégal"
  ],
  "Immobilier": [
    "Immobilier"
  ],
  "Intelligence Artificielle": [
    "Intelligence Artificielle"
  ],
  "Intérêts / Loisirs": [
    "Astrology",
    "Arts et Culture",
    "Cinéma",
    "Célébrités",
    "Enfance",
    "Humour",
    "Informatique et Technologies",
    "Jeux",
    "Jeux Vidéo, Jeux en Ligne",
    "Jeux, Jouets",
    "Loisirs, Hobbies, Passions",
    "Mode, Beauté, Bien-Etre, Décoration",
    "Météo",
    "Sciences, Recherches",
    "Sports"
  ],
  "Jeux d'argent": [
    "Jeux d'Argent Condamnés par la Loi Française",
    "Jeux d'Argent, Micro Paiement, Loteries",
    "Site de jeux d'argent en ligne , casino"
  ],
  "Moteur de recherche": [
    "Portails et Moteurs de Recherche Généralistes"
  ],
  "Téléchargement de Fichiers": [
    "Téléchargement de Fichiers"
  ],
  "Streaming, Télévision, Radio": [
    "Audio et Vidéo",
    "UT1 - Audio-video",
    "Sites de Partage de Vidéo"
  ],
  "Médias, Actualités": [
    "Médias, Actualités",
    "UT1 - Radio"
  ],
  "Navigation / Transport": [
    "Guides, Plans, Etat des Routes"
  ],
  "Occulte / Secte": [
    "Religions Non Traditionnelles, Occultes, Sectes",
    "UT1 - Sect"
  ],
  "Petites annonces": [
    "Petites Annonces"
  ],
  "Politique / Droit / Social": [
    "Comités d'Entreprises",
    "Droit Social",
    "Droit, Fiscalité",
    "Organisations Politiques et Sociales",
    "Sujets de Société"
  ],
  "Pornographie / Nudité / Images à caractère sexuel": [
    "Lingerie, Maillots de Bains",
    "Nudité",
    "Pornographie Condamnée par la Loi Française",
    "Sexe, Pornographie",
    "Sexualité",
    "Sites parlant d'éducation sexuelle pouvant être détéctés comme pornographiques",
    "UT1 - Adult",
    "UT1 - Mixed_adult"
  ],
  "Prise de contrôle à distance": [
    "Prise en Main à Distance, Outils de Collaboration en Ligne"
  ],
  "Publicité": [
    "Navigation Rémunérée",
    "Photographie, Bases de Données d'Images",
    "Publicité",
    "UT1 - Publicité"
  ],
  "Religion": [
    "Religions Traditionnelles"
  ],
  "Réseaux sociaux": [
    "Réseaux Sociaux"
  ],
  "Santé": [
    "Santé"
  ],
  "Sites / Applications de rencontre": [
    "Sites de rencontres"
  ],
  "Services / Sites d'entreprises": [
    "Communication d'Entreprise",
    "Services aux Entreprises",
    "Services aux Particuliers",
    "Site Interne",
    "UT1 - Marketingware"
  ],
  "Traduction": [
    "Traducteurs"
  ],
  "Téléphonie Mobile": [
    "Téléphonie Mobile, Logos, Sonneries",
    "UT1 - Mobile-phone"
  ],
  "VPNs, Filtres, Proxies et Redirection": [
    "Proxies, Redirecteurs",
    "Sites qui permettent de contourner des blacklists de filtres"
  ],
  "Virus / Piratage informatique": [
    "Sites de phishing de pièges bancaire ou autres",
    "Sites de piratage et d'agressions informatiques",
    "Vente d'Armes Condamnée par la Loi Française",
    "Virus, Spywares, Phishing, Codes Malicieux"
  ],
  "Voitures, Mécaniques": [
    "Voitures, Mécaniques"
  ],
  "Voyage / tourisme / Sortie": [
    "Sorties, Soirées, Concerts",
    "Tourisme, Hôtels, Restaurants"
  ]
}
"#;